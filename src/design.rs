//! The design-system catalog, pools, the safe-by-default draw, and the lock.
//!
//! The catalog is **vendored from Open Design** as each system's full curated agent package
//! (`design/systems/<brand>/` — `DESIGN.md`, `USAGE.md`, the machine-readable `tokens.css`,
//! `design-tokens.json`, `components.html`, …; see `design/systems/PROVENANCE`). It is
//! framework-generic — a design system is an abstract, stack-neutral bundle — so it lives once here,
//! not per stack. lila does NOT re-derive tokens or author components: the per-stack render simply
//! copies upstream's curated `tokens.css` + reference components into the app. lila's contribution is
//! the machinery *around* the catalog:
//!
//! - **Three nested pools** (`default` ⊂ `browsable` ⊂ `full`), recorded in `design/systems/INDEX.md`.
//!   The framework only ever draws *blindly* from the tiny `default` pool of safe neutrals; the user
//!   reaches `browsable` on request and `full` only via an explicit pin.
//! - **The draw** ([`draw_system`]) turns `LILA_DESIGN` (`random` / `random:<seed>` / `<brand>`) into a
//!   concrete system, restricting a blind `random` to the default pool. Deterministic given a seed.
//! - **The lock** ([`DesignLock`]) — a committed `design.lock` that doubles as the selection-flow state
//!   machine (`source`: default | invited | chosen | pinned).
//!
//! Resolution mirrors [`crate::stack::stacks_dir`]: CWD first (dev / on-box / tests run from the repo
//! root), then the crate manifest dir.

use std::path::{Path, PathBuf};

use anyhow::{Context, anyhow};

/// The three nested pools. `default` ⊂ `browsable` ⊂ `full`; a system's INDEX row records its
/// *narrowest* membership, and [`Pool::admits`] applies the nesting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pool {
    /// The ~3 safe neutrals the framework may draw blindly.
    Default,
    /// The curated slice the look-change flow offers on request (includes the default systems).
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
}
