//! `lila stack <name>` — print a stack profile's fields as shell assignments, the bridge that lets
//! `bin/new-app` and `bootstrap.sh` consume the data-driven stack contract without parsing TOML in
//! bash. Consumers do `eval "$(lila stack <name>)"` and read the `LILA_STACK_*` variables.
//!
//! Values are single-quoted so a literal `${APP_PORT}` in the serve command survives the `eval`
//! (systemd / the eval probe expands it later). Any embedded single quotes are escaped.

use crate::stack::StackProfile;

/// Print the named stack's fields as `LILA_STACK_*` shell assignments. Exit 0 on success, 1 if the
/// stack can't be loaded.
pub fn run(name: &str) -> i32 {
    let profile = match StackProfile::load(name) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("could not load stack '{name}': {e:#}");
            return 1;
        }
    };

    // `mise use -g` arguments: `tool@version` per pin (sorted for determinism — BTreeMap iterates so).
    let toolchain = profile
        .toolchain
        .iter()
        .map(|(tool, ver)| format!("{tool}@{ver}"))
        .collect::<Vec<_>>()
        .join(" ");
    // `Environment=K=V` lines for the serve unit, one per pair, newline-separated.
    let serve_env = profile
        .serve_env
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("\n");

    println!("{}", assign("LILA_STACK_NAME", &profile.name));
    println!("{}", assign("LILA_STACK_DISPLAY", &profile.display));
    println!(
        "{}",
        assign(
            "LILA_STACK_SCAFFOLD",
            &profile.scaffold_script.to_string_lossy()
        )
    );
    println!("{}", assign("LILA_STACK_SERVE_EXEC", &profile.serve_exec));
    println!("{}", assign("LILA_STACK_SERVE_ENV", &serve_env));
    println!("{}", assign("LILA_STACK_TOOLCHAIN", &toolchain));
    0
}

/// `KEY='value'` with single quotes escaped so the line is safe to `eval`.
fn assign(key: &str, value: &str) -> String {
    let escaped = value.replace('\'', r"'\''");
    format!("{key}='{escaped}'")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assign_escapes_single_quotes() {
        assert_eq!(assign("K", "a'b"), r"K='a'\''b'");
    }

    #[test]
    fn rails_pwa_loads_for_the_shell_bridge() {
        // The command must resolve the in-repo default stack (smoke: exit 0).
        assert_eq!(run("rails-pwa"), 0);
        assert_eq!(run("does-not-exist"), 1);
    }
}
