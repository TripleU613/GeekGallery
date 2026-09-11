//! Cloudflare D1 client, over D1's HTTP API.
//!
//! The app runs as a native binary on a plain server, not a Cloudflare Worker,
//! so there is no in-process binding to D1 — every query is one HTTPS round
//! trip. sqlx has no driver for D1's wire format (it isn't Postgres/MySQL/
//! SQLite-over-TCP; it's a REST API), hence this hand-rolled client rather than
//! an existing crate.
//!
//! The consequence that shapes everything downstream: **there is no
//! cross-request transaction.** A `batch` is atomic (rolled back together on a
//! genuine SQLite exception — a constraint or trigger failure — per Cloudflare's
//! docs), but it is a fixed list of statements decided before the call; nothing
//! in it can branch on a value read earlier in the same batch, and two separate
//! HTTP calls are never atomic together. `db::toggle_like` is written the way it
//! is because of this constraint, not by choice — see the comment there.

use serde::de::DeserializeOwned;
use serde_json::Value;

pub struct D1 {
    http: reqwest::Client,
    endpoint: String,
    token: String,
}

/// One statement in a batch: SQL plus its positional `?` parameters.
pub struct Stmt {
    pub sql: String,
    pub params: Vec<Value>,
}

impl Stmt {
    pub fn new(sql: impl Into<String>, params: Vec<Value>) -> Self {
        Self {
            sql: sql.into(),
            params,
        }
    }
}

#[derive(serde::Deserialize)]
struct D1Response {
    success: bool,
    #[serde(default)]
    errors: Vec<D1Error>,
    #[serde(default)]
    result: Vec<D1StatementResult>,
}

#[derive(serde::Deserialize)]
struct D1Error {
    code: i64,
    message: String,
}

#[derive(serde::Deserialize)]
struct D1StatementResult {
    #[serde(default)]
    results: Vec<Value>,
}

impl D1 {
    pub fn from_env() -> anyhow::Result<Self> {
        let account = std::env::var("CF_ACCOUNT_ID")
            .map_err(|_| anyhow::anyhow!("CF_ACCOUNT_ID is not set"))?;
        let database = std::env::var("CF_D1_DATABASE_ID")
            .map_err(|_| anyhow::anyhow!("CF_D1_DATABASE_ID is not set"))?;
        let token = std::env::var("CF_D1_API_TOKEN")
            .map_err(|_| anyhow::anyhow!("CF_D1_API_TOKEN is not set"))?;

        Ok(Self {
            http: reqwest::Client::new(),
            endpoint: format!(
                "https://api.cloudflare.com/client/v4/accounts/{account}/d1/database/{database}/query"
            ),
            token,
        })
    }

    /// A single statement. For an INSERT/UPDATE/DELETE the row set is usually
    /// empty unless the statement has a `RETURNING` clause.
    pub async fn exec(&self, sql: &str, params: Vec<Value>) -> anyhow::Result<Vec<Value>> {
        let mut rows = self.batch(vec![Stmt::new(sql, params)]).await?;
        Ok(rows.pop().unwrap_or_default())
    }

    /// `exec`, deserialized into `T` per row.
    pub async fn query<T: DeserializeOwned>(
        &self,
        sql: &str,
        params: Vec<Value>,
    ) -> anyhow::Result<Vec<T>> {
        let rows = self.exec(sql, params).await?;
        rows.into_iter()
            .map(|v| serde_json::from_value(v).map_err(|e| anyhow::anyhow!("bad row shape: {e}")))
            .collect()
    }

    /// Run several statements as one D1 batch. D1 executes a batch atomically —
    /// see the module docs for exactly what that does and does not guarantee.
    /// Returns one result-row-set per statement, in order.
    pub async fn batch(&self, stmts: Vec<Stmt>) -> anyhow::Result<Vec<Vec<Value>>> {
        let body: Vec<Value> = stmts
            .into_iter()
            .map(|s| serde_json::json!({"sql": s.sql, "params": s.params}))
            .collect();

        let resp = self
            .http
            .post(&self.endpoint)
            .bearer_auth(&self.token)
            .json(&serde_json::json!({"batch": body}))
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("D1 request failed: {e}"))?;

        let status = resp.status();
        let parsed: D1Response = resp
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("D1 returned an unreadable response ({status}): {e}"))?;

        if !parsed.success {
            let msg = parsed
                .errors
                .iter()
                .map(|e| format!("[{}] {}", e.code, e.message))
                .collect::<Vec<_>>()
                .join("; ");
            anyhow::bail!(
                "D1 query failed: {}",
                if msg.is_empty() {
                    "unknown error".into()
                } else {
                    msg
                }
            );
        }

        Ok(parsed.result.into_iter().map(|r| r.results).collect())
    }
}
