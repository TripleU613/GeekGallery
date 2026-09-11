/** @type {import('tailwindcss').Config} */
module.exports = {
  // Classes live inside Leptos `view!` macros in .rs files, not in .html/.jsx,
  // so the scanner has to read Rust source or every utility gets purged.
  content: ["./src/**/*.rs"],
  theme: {
    extend: {
      // The palette. Every token is a CSS variable the document `<head>`
      // sets from the flavor (see `flavor::Theme::css`), so `bg-surface` is
      // one class in every deployment and only the value behind it moves.
      // The fallbacks are the default palette, for a page that somehow has
      // no `:root` block -- cool near-black neutrals with a periwinkle
      // accent, every text token measured over 4.5:1 on the surfaces it
      // lands on.
      //
      // Written as `rgb(var(--c-x) / <alpha-value>)` so opacity modifiers
      // (`bg-bg/50`) keep working; the four accent tints are fixed alphas of
      // the accent variable, which is what lets one `THEME_ACCENT` recolour
      // the hairlines and washes with it. `public_route::EMBED_CSS` is built
      // from the same variables at runtime for the framable card, which loads
      // no stylesheet.
      colors: {
        bg: "rgb(var(--c-bg, 10 11 15) / <alpha-value>)",
        surface: {
          DEFAULT: "rgb(var(--c-surface, 16 18 26) / <alpha-value>)",
          raised: "rgb(var(--c-surface-raised, 23 26 37) / <alpha-value>)",
          hover: "rgb(var(--c-surface-hover, 30 34 48) / <alpha-value>)",
        },
        line: {
          DEFAULT: "rgb(var(--c-line, 43 48 66) / <alpha-value>)",
          strong: "rgb(var(--c-line-strong, 61 67 88) / <alpha-value>)",
        },
        ink: {
          DEFAULT: "rgb(var(--c-ink, 242 244 248) / <alpha-value>)",
          2: "rgb(var(--c-ink-2, 170 176 192) / <alpha-value>)",
          3: "rgb(var(--c-ink-3, 127 134 152) / <alpha-value>)",
        },
        // The accent is a scale, not one value repeated: a wash, a hairline
        // and a muted text tone are three different jobs and reusing DEFAULT
        // for all of them forces the choice between "invisible" and
        // "shouting".
        accent: {
          DEFAULT: "rgb(var(--c-accent, 154 164 255) / <alpha-value>)",
          hover: "rgb(var(--c-accent-hover, 176 184 255) / <alpha-value>)",
          active: "rgb(var(--c-accent-active, 122 132 245) / <alpha-value>)",
          // Pressed-chip fill.
          soft: "rgb(var(--c-accent, 154 164 255) / 0.14)",
          // Large, low-stakes washes: the rank-1 leaderboard row, the
          // empty-state icon badge. Deliberately too faint to carry meaning on
          // its own -- everything sitting on it is also marked some other way.
          veil: "rgb(var(--c-accent, 154 164 255) / 0.06)",
          // The quiet accent hairline: card and chip hover edges.
          line: "rgb(var(--c-accent, 154 164 255) / 0.30)",
          // Text that should read as the accent without shouting.
          muted: "rgb(var(--c-accent-muted, 128 137 214) / <alpha-value>)",
          border: "rgb(var(--c-accent, 154 164 255) / 0.75)",
          ink: "rgb(var(--c-accent-ink, 10 11 15) / <alpha-value>)",
        },
        danger: "rgb(var(--c-danger, 255 107 122) / <alpha-value>)",
        ok: "rgb(var(--c-ok, 78 227 160) / <alpha-value>)",
      },
      borderRadius: {
        sm: "8px",
        DEFAULT: "10px",
        lg: "13px",
      },
      spacing: {
        // Chrome heights, referenced by the content padding that clears them.
        topbar: "56px",
        "topbar-lg": "60px",
        bottomnav: "58px",
        // Tailwind's default spacing scale jumps 12 -> 14; the empty-state
        // badges want the step in between.
        13: "3.25rem",
      },
      fontFamily: {
        sans: [
          "Inter",
          "Geist",
          "system-ui",
          "-apple-system",
          "BlinkMacSystemFont",
          "Segoe UI",
          "Roboto",
          "sans-serif",
        ],
      },
      maxWidth: {
        content: "1320px",
        wide: "1800px",
      },
      // Motion lives here as config keyframes rather than hand-written CSS so
      // it comes out as real utilities (`animate-rise-in`) the scanner can see
      // and purge like anything else.
      keyframes: {
        shimmer: {
          "0%": { backgroundPosition: "-150% 0" },
          "100%": { backgroundPosition: "250% 0" },
        },
        "rise-in": {
          "0%": { opacity: "0", transform: "translateY(10px)" },
          "100%": { opacity: "1", transform: "none" },
        },
        "fade-in": {
          "0%": { opacity: "0" },
          "100%": { opacity: "1" },
        },
        // The upload success card: the medallion lands, then the tick draws.
        pop: {
          "0%": { opacity: "0", transform: "scale(.6)" },
          "60%": { opacity: "1", transform: "scale(1.08)" },
          "100%": { opacity: "1", transform: "scale(1)" },
        },
        draw: {
          "0%": { strokeDashoffset: "1" },
          "100%": { strokeDashoffset: "0" },
        },
      },
      animation: {
        shimmer: "shimmer 1.9s cubic-bezier(.4, 0, .6, 1) infinite",
        // The `both` fill-mode on the two entrance animations is load-bearing,
        // not decoration. The global prefers-reduced-motion block clamps
        // animation-duration to 0.01ms; without `both`, an element whose
        // animation has effectively already finished can be left showing the
        // 0% frame -- opacity 0 -- for exactly the users who asked for less
        // motion.
        "rise-in": "rise-in .34s cubic-bezier(.16, 1, .3, 1) both",
        "fade-in": "fade-in .22s ease-out both",
        pop: "pop .5s cubic-bezier(.16, 1, .3, 1) both",
        // Starts once the medallion has landed; `pathLength=1` on the path
        // makes the dash arithmetic unit-free.
        draw: "draw .45s .22s ease-out both",
      },
      backgroundImage: {
        // The skeleton's travelling highlight, in the accent rather than white,
        // so a loading screen is a coloured moment instead of grey boxes.
        sheen:
          "linear-gradient(100deg, transparent 20%, rgb(var(--c-accent, 154 164 255) / 0.08) 45%, transparent 70%)",
      },
    },
  },
  plugins: [],
};
