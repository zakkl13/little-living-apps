//! The design-system catalog, pools, the safe-by-default draw, the lock, and the token render.
//!
//! The catalog is **vendored from Open Design** (`design/systems/<brand>/DESIGN.md`, see
//! `design/systems/PROVENANCE`). It is framework-generic — a `DESIGN.md` is an abstract, stack-neutral
//! bundle, so it lives once here, not per stack. lila's contribution is the machinery *around* the
//! catalog:
//!
//! - **Three nested pools** (`default` ⊂ `browsable` ⊂ `full`), recorded in `design/systems/INDEX.md`.
//!   The framework only ever draws *blindly* from the tiny `default` pool of safe neutrals; the user
//!   reaches `browsable` on request and `full` only via an explicit pin.
//! - **The draw** ([`draw_system`]) turns `LILA_DESIGN` (`random` / `random:<seed>` / `<brand>`) into a
//!   concrete system, restricting a blind `random` to the default pool. Deterministic given a seed.
//! - **The lock** ([`DesignLock`]) — a committed `design.lock` that doubles as the selection-flow state
//!   machine (`source`: default | invited | chosen | pinned).
//! - **The token render** ([`render_tokens_css`]) — the stack-neutral half of the per-stack render:
//!   a `DESIGN.md`'s color + type sections emitted as CSS custom properties (+ a dark-mode block).
//!
//! Resolution mirrors [`crate::stack::stacks_dir`]: CWD first (dev / on-box / tests run from the repo
//! root), then the crate manifest dir.

use std::path::{Path, PathBuf};

use anyhow::{Context, anyhow};
use regex::Regex;

/// The three nested pools. `default` ⊂ `browsable` ⊂ `full`; a system's INDEX row records its
/// *narrowest* membership, and [`Pool::admits`] applies the nesting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pool {
    /// The ~3 safe neutrals the framework may draw blindly.
    Default,
    /// The curated slice the selection skill offers on request (includes the default systems).
    Browsable,
    /// Every vendored system; reachable only by an explicit `LILA_DESIGN=<brand>` pin.
    Full,
}

impl Pool {
    /// Parse a pool label from an INDEX row (`default` / `browsable` / `full`).
    pub fn parse(s: &str) -> Option<Pool> {
        match s.trim() {
            "default" => Some(Pool::Default),
            "browsable" => Some(Pool::Browsable),
            "full" => Some(Pool::Full),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Pool::Default => "default",
            Pool::Browsable => "browsable",
            Pool::Full => "full",
        }
    }

    /// Does `self` (a *queried* pool) admit a system whose *narrowest* pool is `member`? Pools nest, so
    /// `browsable` admits both browsable and default systems, and `full` admits everything.
    pub fn admits(self, member: Pool) -> bool {
        let rank = |p: Pool| match p {
            Pool::Default => 0,
            Pool::Browsable => 1,
            Pool::Full => 2,
        };
        rank(member) <= rank(self)
    }
}

/// One catalog row, parsed from `design/systems/INDEX.md`.
#[derive(Debug, Clone)]
pub struct SystemEntry {
    pub brand: String,
    pub category: String,
    pub pool: Pool,
    pub voice: String,
}

/// The committed `design.lock` — the active system *and* the selection-flow state machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesignLock {
    /// The active system's `design/systems/<brand>` name.
    pub brand: String,
    /// Which pool it came from.
    pub pool: Pool,
    /// The state machine: `default` (blind draw) | `invited` | `chosen` | `pinned`.
    pub source: String,
    /// The draw seed (for reproducibility).
    pub seed: u64,
    /// Upstream Open Design provenance (the pinned catalog commit).
    pub commit: String,
}

impl DesignLock {
    /// Serialize to the on-disk `design.lock` TOML body.
    pub fn to_toml(&self) -> String {
        format!(
            "brand  = \"{}\"\npool   = \"{}\"\nsource = \"{}\"\nseed   = {}\ncommit = \"{}\"\n",
            self.brand,
            self.pool.as_str(),
            self.source,
            self.seed,
            self.commit
        )
    }

    /// Parse a `design.lock` TOML body (tolerant of field order / whitespace).
    pub fn parse(body: &str) -> anyhow::Result<DesignLock> {
        let brand = lock_field(body, "brand").context("design.lock missing brand")?;
        let pool = lock_field(body, "pool")
            .and_then(|p| Pool::parse(&p))
            .context("design.lock missing/invalid pool")?;
        let source = lock_field(body, "source").context("design.lock missing source")?;
        let seed = lock_field(body, "seed")
            .and_then(|s| s.parse().ok())
            .context("design.lock missing/invalid seed")?;
        let commit = lock_field(body, "commit").unwrap_or_default();
        Ok(DesignLock {
            brand,
            pool,
            source,
            seed,
            commit,
        })
    }
}

/// Pull a `key = "value"` or `key = 123` field out of a lock/TOML body.
fn lock_field(body: &str, key: &str) -> Option<String> {
    body.lines()
        .filter_map(|l| l.split_once('='))
        .find(|(k, _)| k.trim() == key)
        .map(|(_, v)| v.trim().trim_matches('"').to_string())
}

/// Resolve the catalog dir (`design/systems`): CWD first, then the crate manifest dir.
pub fn catalog_dir() -> PathBuf {
    let from_cwd = std::env::current_dir()
        .unwrap_or_default()
        .join("design/systems");
    if from_cwd.exists() {
        return from_cwd;
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("design/systems")
}

/// The pinned upstream commit, read from `design/systems/PROVENANCE` (`Commit:` line). Empty if absent.
pub fn catalog_commit(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("PROVENANCE"))
        .ok()
        .and_then(|body| {
            // Tolerate a leading "- " bullet: find the "Commit:" marker anywhere on the line.
            body.lines()
                .find_map(|l| l.split_once("Commit:"))
                .map(|(_, rest)| rest.trim().to_string())
        })
        .unwrap_or_default()
}

/// Parse `design/systems/INDEX.md` into catalog rows (skips the header + non-data lines).
pub fn load_index(dir: &Path) -> anyhow::Result<Vec<SystemEntry>> {
    let path = dir.join("INDEX.md");
    let body =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    Ok(body.lines().filter_map(parse_index_row).collect())
}

/// Parse one INDEX table row (`| brand | category | pool | voice |`) into an entry, or `None` for the
/// header / separator / prose lines.
fn parse_index_row(line: &str) -> Option<SystemEntry> {
    let line = line.trim();
    if !line.starts_with('|') {
        return None;
    }
    let cols: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
    if cols.len() != 4 {
        return None;
    }
    let pool = Pool::parse(cols[2])?; // skips the "| brand | category | pool | voice |" header
    Some(SystemEntry {
        brand: cols[0].to_string(),
        category: cols[1].to_string(),
        pool,
        voice: cols[3].to_string(),
    })
}

/// The outcome of resolving `LILA_DESIGN` against the catalog.
#[derive(Debug, Clone)]
pub struct Draw {
    pub brand: String,
    /// Absolute path to the chosen system's `DESIGN.md`.
    pub design_md: PathBuf,
    pub pool: Pool,
    /// `default` for a blind random draw, `pinned` for an explicit `<brand>`.
    pub source: String,
    pub seed: u64,
}

/// Turn a `LILA_DESIGN` choice into a concrete system, bounded by pool:
/// - `random`         → uniform draw from the **default pool** (seed from the clock; non-reproducible).
/// - `random:<seed>`  → reproducible draw from the **default pool**.
/// - `<brand>`        → pin a specific system from **any** pool (the escape hatch); `source = pinned`.
///
/// A blind `random` can therefore only ever land on a safe neutral — the user owns any reach beyond.
pub fn draw_system(dir: &Path, choice: &str) -> anyhow::Result<Draw> {
    let entries = load_index(dir)?;
    match parse_choice(choice) {
        Choice::Random(seed) => draw_random(dir, &entries, seed),
        Choice::Pinned(brand) => pin_brand(dir, &entries, &brand),
    }
}

enum Choice {
    Random(u64),
    Pinned(String),
}

/// `random` → clock seed; `random:<n>` → fixed seed; anything else → a brand pin.
fn parse_choice(choice: &str) -> Choice {
    let choice = choice.trim();
    if choice == "random" {
        return Choice::Random(clock_seed());
    }
    if let Some(seed) = choice.strip_prefix("random:") {
        return Choice::Random(seed.trim().parse().unwrap_or_else(|_| fnv1a(seed)));
    }
    Choice::Pinned(choice.to_string())
}

/// Draw uniformly from the default pool for `seed`. Deterministic: the same seed always yields the
/// same brand, and the brand is always a member of the default pool.
fn draw_random(dir: &Path, entries: &[SystemEntry], seed: u64) -> anyhow::Result<Draw> {
    let mut pool: Vec<&SystemEntry> = entries.iter().filter(|e| e.pool == Pool::Default).collect();
    pool.sort_by(|a, b| a.brand.cmp(&b.brand)); // stable order so the draw is reproducible
    if pool.is_empty() {
        return Err(anyhow!("the default pool is empty in {}", dir.display()));
    }
    let pick = pool[(seed % pool.len() as u64) as usize];
    Ok(Draw {
        brand: pick.brand.clone(),
        design_md: design_md_path(dir, &pick.brand)?,
        pool: Pool::Default,
        source: "default".to_string(),
        seed,
    })
}

/// Pin an explicit brand from any pool. `source = pinned` (suppresses the invitation).
fn pin_brand(dir: &Path, entries: &[SystemEntry], brand: &str) -> anyhow::Result<Draw> {
    let pool = entries
        .iter()
        .find(|e| e.brand == brand)
        .map(|e| e.pool)
        .unwrap_or(Pool::Full);
    Ok(Draw {
        brand: brand.to_string(),
        design_md: design_md_path(dir, brand)?,
        pool,
        source: "pinned".to_string(),
        seed: fnv1a(brand),
    })
}

/// Resolve + verify a brand's `DESIGN.md` path (so a bad pin fails loudly at draw time).
fn design_md_path(dir: &Path, brand: &str) -> anyhow::Result<PathBuf> {
    let path = dir.join(brand).join("DESIGN.md");
    if !path.exists() {
        return Err(anyhow!(
            "unknown design system \"{brand}\" (no {})",
            path.display()
        ));
    }
    Ok(path)
}

/// A clock-derived seed for a non-reproducible `random` draw.
fn clock_seed() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E37_79B9_7F4A_7C15)
}

/// FNV-1a over a string — a tiny, dependency-free stable hash for turning a label into a seed.
fn fnv1a(s: &str) -> u64 {
    let mut hash: u64 = 0xCBF2_9CE4_8422_2325;
    for b in s.bytes() {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
    }
    hash
}

// ---- token render (the stack-neutral half of the per-stack render) ------------------------------

/// Render a `DESIGN.md`'s color + type sections into CSS custom properties on `:root`, plus a
/// `prefers-color-scheme: dark` block overriding the same token names. Stack-neutral: a stack writes
/// this into its `tokens_path` and layers its own (idiomatic) component partials on top.
///
/// Parsing is deliberately tolerant — Open Design's `DESIGN.md` is prose, not a strict schema — so any
/// role/font it can't find falls back to a safe neutral default rather than failing the render.
pub fn render_tokens_css(design_md: &str) -> String {
    let roles = extract_color_roles(design_md);
    let c = |fallbacks: &[&str], default: &str| -> String {
        pick_role(&roles, fallbacks).unwrap_or_else(|| default.to_string())
    };
    let bg = c(&["background", "bg", "surface", "neutral"], "#fafafa");
    let surface = c(&["surface", "card", "background"], "#ffffff");
    let fg = c(&["foreground", "text", "ink", "body"], "#111111");
    let muted = c(&["muted", "secondary", "subtle", "caption"], "#6b6b6b");
    let border = c(&["border", "divider", "line"], "#e5e5e5");
    let accent = c(&["accent", "primary", "brand", "cta"], "#2f6feb");
    let success = c(&["success", "positive"], "#17a34a");
    let warning = c(&["warn", "warning", "caution"], "#eab308");
    let danger = c(&["danger", "error", "destructive", "negative"], "#dc2626");
    let on_accent = contrast_color(&accent);

    let fonts = extract_fonts(design_md);
    let sans = fonts
        .sans
        .unwrap_or_else(|| "system-ui, -apple-system, 'Segoe UI', Roboto, sans-serif".to_string());
    let serif = fonts
        .serif
        .unwrap_or_else(|| "Georgia, 'Times New Roman', serif".to_string());
    let mono = fonts
        .mono
        .unwrap_or_else(|| "ui-monospace, 'JetBrains Mono', 'SF Mono', monospace".to_string());

    format!(
        "{HEADER}:root {{\n\
         {INDENT}/* color — extracted from the system's palette, with safe neutral fallbacks */\n\
         {INDENT}--color-bg: {bg};\n\
         {INDENT}--color-surface: {surface};\n\
         {INDENT}--color-fg: {fg};\n\
         {INDENT}--color-muted: {muted};\n\
         {INDENT}--color-border: {border};\n\
         {INDENT}--color-accent: {accent};\n\
         {INDENT}--color-on-accent: {on_accent};\n\
         {INDENT}--color-success: {success};\n\
         {INDENT}--color-warning: {warning};\n\
         {INDENT}--color-danger: {danger};\n\
         {INDENT}/* type */\n\
         {INDENT}--font-sans: {sans};\n\
         {INDENT}--font-serif: {serif};\n\
         {INDENT}--font-mono: {mono};\n\
         {INDENT}--text-xs: 0.75rem; --text-sm: 0.875rem; --text-base: 1rem; --text-lg: 1.25rem;\n\
         {INDENT}--text-xl: 1.5rem; --text-2xl: 2rem; --text-3xl: 3rem;\n\
         {INDENT}--leading-body: 1.5; --leading-tight: 1.2; --measure: 68ch;\n\
         {INDENT}/* space / radius / elevation / motion — coherent neutral scale */\n\
         {INDENT}--space-1: 0.25rem; --space-2: 0.5rem; --space-3: 0.75rem; --space-4: 1rem;\n\
         {INDENT}--space-6: 1.5rem; --space-8: 2rem; --space-12: 3rem; --space-16: 4rem;\n\
         {INDENT}--radius-sm: 6px; --radius: 10px; --radius-lg: 16px; --radius-pill: 999px;\n\
         {INDENT}--container: 1100px;\n\
         {INDENT}--shadow-1: 0 1px 2px rgba(0,0,0,.06), 0 1px 1px rgba(0,0,0,.04);\n\
         {INDENT}--shadow-2: 0 4px 12px rgba(0,0,0,.10);\n\
         {INDENT}--ease: cubic-bezier(.2,.7,.3,1); --dur: 160ms;\n\
         }}\n\n\
         @media (prefers-color-scheme: dark) {{\n\
         {INDENT}:root {{\n\
         {INDENT}{INDENT}--color-bg: #111317;\n\
         {INDENT}{INDENT}--color-surface: #1a1d23;\n\
         {INDENT}{INDENT}--color-fg: #f5f5f5;\n\
         {INDENT}{INDENT}--color-muted: #9ca3af;\n\
         {INDENT}{INDENT}--color-border: #2a2e37;\n\
         {INDENT}{INDENT}--color-accent: {accent};\n\
         {INDENT}{INDENT}--color-on-accent: {on_accent};\n\
         {INDENT}}}\n\
         }}\n",
    )
}

const INDENT: &str = "  ";
const HEADER: &str = "/* tokens.css — rendered from the locked design system (design.lock).\n\
   Generated at scaffold; do not hand-edit individual values — change the look via the design skill,\n\
   which re-renders this file. Build views by referencing these custom properties, never raw hex. */\n";

/// All `**Role:** \`#hex\`` pairs in the document, lowercased role → hex (first occurrence wins).
fn extract_color_roles(body: &str) -> Vec<(String, String)> {
    let re = role_hex_re();
    let mut out: Vec<(String, String)> = Vec::new();
    for cap in re.captures_iter(body) {
        let role = cap[1].trim().trim_end_matches(':').to_lowercase();
        let hex = cap[2].to_lowercase();
        if !out.iter().any(|(r, _)| *r == role) {
            out.push((role, hex));
        }
    }
    out
}

/// `**Role:**` … `` `#hex` `` on one line. Authored constant ⇒ a malformed pattern is a build bug.
#[allow(clippy::expect_used)]
fn role_hex_re() -> Regex {
    // The role-name class allows parens so annotated labels like `**Accent (primary):**` are captured.
    Regex::new(r"\*\*\s*([A-Za-z][A-Za-z0-9 /&._()-]*?)\s*:?\s*\*\*[^`\n]*`(#[0-9A-Fa-f]{3,8})`")
        .expect("role/hex regex must be valid")
}

/// First role whose name contains any of `names` (in priority order), returning its hex.
fn pick_role(roles: &[(String, String)], names: &[&str]) -> Option<String> {
    names.iter().find_map(|want| {
        roles
            .iter()
            .find(|(role, _)| role.contains(want))
            .map(|(_, hex)| hex.clone())
    })
}

/// The three font roles a render needs, each an optional extracted stack.
struct Fonts {
    sans: Option<String>,
    serif: Option<String>,
    mono: Option<String>,
}

/// Extract sans / serif / mono font stacks from the typography section. Scoped to that section (so a
/// stray backtick span elsewhere can't masquerade as a font) and matched on real font signals — a
/// known generic family or a quoted family name — never loose prose.
fn extract_fonts(body: &str) -> Fonts {
    let section = typography_slice(body);
    let stacks: Vec<String> = font_re()
        .captures_iter(section)
        .map(|c| c[1].trim().to_string())
        .collect();
    let has = |s: &str, kw: &str| s.to_lowercase().contains(kw);
    Fonts {
        mono: stacks.iter().find(|s| has(s, "mono")).cloned(),
        // serif but not sans-serif (which is a sans signal).
        serif: stacks
            .iter()
            .find(|s| has(s, "serif") && !has(s, "sans"))
            .cloned(),
        sans: stacks
            .iter()
            .find(|s| has(s, "sans") || has(s, "system-ui") || has(s, "-apple-system"))
            .cloned(),
    }
}

/// The typography section body (from a "Typograph…" heading to the next `## ` heading), or the whole
/// document if no such heading is found.
fn typography_slice(body: &str) -> &str {
    let start = match body.to_lowercase().find("typograph") {
        Some(i) => body[..i].rfind("\n## ").map(|n| n + 1).unwrap_or(i),
        None => return body,
    };
    let rest = &body[start..];
    let end = rest[3..].find("\n## ").map(|i| i + 3).unwrap_or(rest.len());
    &rest[..end]
}

/// A backtick-quoted font stack: contains a generic family keyword or a quoted family name. Authored
/// constant ⇒ a malformed pattern is a build bug.
#[allow(clippy::expect_used)]
fn font_re() -> Regex {
    Regex::new(r"`([^`\n]*(?:sans-serif|serif|monospace|system-ui|-apple-system|'[^'`]+')[^`\n]*)`")
        .expect("font regex must be valid")
}

/// Pick black or white as the readable foreground over `hex` (WCAG relative-luminance heuristic).
fn contrast_color(hex: &str) -> String {
    match parse_rgb(hex) {
        Some((r, g, b)) => {
            let lum = 0.2126 * srgb(r) + 0.7152 * srgb(g) + 0.0722 * srgb(b);
            if lum > 0.4 { "#111111" } else { "#ffffff" }.to_string()
        }
        None => "#ffffff".to_string(),
    }
}

/// Parse `#rgb` or `#rrggbb` into 0–255 components.
fn parse_rgb(hex: &str) -> Option<(u8, u8, u8)> {
    let h = hex.trim_start_matches('#');
    let full = match h.len() {
        3 => h.chars().flat_map(|c| [c, c]).collect::<String>(),
        6 | 8 => h[..6].to_string(),
        _ => return None,
    };
    let byte = |i: usize| u8::from_str_radix(&full[i..i + 2], 16).ok();
    Some((byte(0)?, byte(2)?, byte(4)?))
}

/// sRGB channel to linear light (for the luminance estimate).
fn srgb(c: u8) -> f64 {
    let cs = c as f64 / 255.0;
    if cs <= 0.039_28 {
        cs / 12.92
    } else {
        ((cs + 0.055) / 1.055).powf(2.4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> PathBuf {
        catalog_dir()
    }

    #[test]
    fn index_parses_and_has_three_default_systems() {
        let entries = load_index(&dir()).expect("INDEX.md loads");
        let defaults: Vec<&str> = entries
            .iter()
            .filter(|e| e.pool == Pool::Default)
            .map(|e| e.brand.as_str())
            .collect();
        assert_eq!(defaults.len(), 3, "exactly three blind-draw neutrals");
        assert!(entries.len() > 100, "the full catalog is vendored");
    }

    #[test]
    fn blind_random_is_bounded_to_the_default_pool() {
        for seed in 0..50u64 {
            let d = draw_system(&dir(), &format!("random:{seed}")).expect("draw");
            assert_eq!(
                d.pool,
                Pool::Default,
                "seed {seed} escaped the default pool"
            );
            assert_eq!(d.source, "default");
        }
    }

    #[test]
    fn same_seed_yields_same_brand() {
        let a = draw_system(&dir(), "random:1234").expect("draw a");
        let b = draw_system(&dir(), "random:1234").expect("draw b");
        assert_eq!(a.brand, b.brand);
        assert_eq!(a.seed, 1234);
    }

    #[test]
    fn pin_reaches_full_pool_and_sets_pinned_source() {
        // A non-default, non-browsable system is reachable only by pin.
        let d = draw_system(&dir(), "stripe").expect("pin stripe");
        assert_eq!(d.brand, "stripe");
        assert_eq!(d.source, "pinned");
        assert!(d.design_md.ends_with("stripe/DESIGN.md"));
    }

    #[test]
    fn unknown_pin_errors() {
        assert!(draw_system(&dir(), "not-a-real-system").is_err());
    }

    #[test]
    fn pool_nesting_admits_narrower_members() {
        assert!(Pool::Browsable.admits(Pool::Default));
        assert!(Pool::Full.admits(Pool::Browsable));
        assert!(!Pool::Default.admits(Pool::Browsable));
    }

    #[test]
    fn lock_round_trips() {
        let lock = DesignLock {
            brand: "default".into(),
            pool: Pool::Default,
            source: "default".into(),
            seed: 99,
            commit: "abc123".into(),
        };
        let parsed = DesignLock::parse(&lock.to_toml()).expect("parse");
        assert_eq!(lock, parsed);
    }

    #[test]
    fn render_emits_tokens_and_dark_mode() {
        let design =
            std::fs::read_to_string(dir().join("default/DESIGN.md")).expect("read default");
        let css = render_tokens_css(&design);
        assert!(
            css.contains("--color-accent: #2f6feb"),
            "extracted cobalt accent"
        );
        assert!(css.contains("--color-bg: #fafafa"));
        assert!(css.contains("--font-sans:"));
        assert!(css.contains("prefers-color-scheme: dark"));
    }

    #[test]
    fn render_extracts_annotated_accent_label() {
        // warm-editorial labels its accent `**Accent (primary):** #C0512F` — the parens must not
        // defeat extraction (else the blind-draw default would lose its real brand color).
        let design = std::fs::read_to_string(dir().join("warm-editorial/DESIGN.md")).expect("read");
        let css = render_tokens_css(&design);
        assert!(
            css.contains("--color-accent: #c0512f"),
            "extracted the terracotta accent, not the fallback"
        );
    }

    #[test]
    fn render_extracts_real_font_stacks_not_prose() {
        // warm-editorial is serif-led: the render must pull its display serif, and never a prose line.
        let design = std::fs::read_to_string(dir().join("warm-editorial/DESIGN.md")).expect("read");
        let css = render_tokens_css(&design);
        let serif_line = css
            .lines()
            .find(|l| l.contains("--font-serif:"))
            .expect("serif token present");
        assert!(
            serif_line.contains("serif"),
            "serif stack ends in a generic family"
        );
        assert!(
            !css.contains("--font-sans: —") && !css.to_lowercase().contains("derived from"),
            "no prose leaked into a font token"
        );
    }

    #[test]
    fn render_falls_back_gracefully_on_empty_input() {
        let css = render_tokens_css("# Nothing\nno tokens here");
        assert!(
            css.contains("--color-accent: #2f6feb"),
            "default accent fallback"
        );
        assert!(css.contains("--color-fg: #111111"));
    }

    #[test]
    fn contrast_picks_readable_foreground() {
        assert_eq!(contrast_color("#ffffff"), "#111111");
        assert_eq!(contrast_color("#000000"), "#ffffff");
    }
}
