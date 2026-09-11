//! Local-disk backend. The development default, and the fallback whenever R2
//! credentials are absent.

use super::{local_path, Backend};

pub struct LocalDisk {
    root: String,
    /// URL prefix the app serves `root` under (see the ServeDir in main.rs).
    url_prefix: String,
}

impl LocalDisk {
    pub fn new(root: impl Into<String>, url_prefix: impl Into<String>) -> Self {
        Self {
            root: root.into(),
            url_prefix: url_prefix.into(),
        }
    }
}

#[async_trait::async_trait]
impl Backend for LocalDisk {
    async fn put(&self, key: &str, bytes: Vec<u8>, _content_type: &str) -> anyhow::Result<()> {
        let path = local_path(&self.root, key);
        if let Some(dir) = path.parent() {
            tokio::fs::create_dir_all(dir).await?;
        }
        tokio::fs::write(&path, bytes).await?;
        Ok(())
    }

    async fn get(&self, key: &str) -> anyhow::Result<Vec<u8>> {
        Ok(tokio::fs::read(local_path(&self.root, key)).await?)
    }

    async fn delete(&self, key: &str) {
        let _ = tokio::fs::remove_file(local_path(&self.root, key)).await;
    }

    fn public_url(&self, key: &str) -> String {
        format!("{}/{}", self.url_prefix.trim_end_matches('/'), key)
    }

    fn name(&self) -> String {
        format!("local disk ({})", self.root)
    }
}
