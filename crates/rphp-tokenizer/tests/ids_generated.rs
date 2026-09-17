//! `src/ids.rs` is generated from the installed PHP (`cargo xtask token-ids`).
//! When a `php` is available, re-derive the table and compare, so a PHP upgrade
//! that adds, renames or renumbers tokens fails loudly instead of drifting.

use std::collections::BTreeMap;
use std::process::Command;

/// Kept textually identical to `PHP_SCRIPT` in `xtask/src/token_ids.rs`.
const PHP_SCRIPT: &str = r#"foreach (get_defined_constants(true)["tokenizer"] as $k => $v) { if (str_starts_with($k, "T_")) echo "C\t$k\t$v\n"; } for ($i = 256; $i <= 450; $i++) { $n = token_name($i); if ($n !== "UNKNOWN") echo "N\t$i\t$n\n"; } echo "V\t", PHP_VERSION, "\n";"#;

fn php_binary() -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("RPHP_PHP") {
        return Some(p.into());
    }
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join("php"))
        .find(|p| p.is_file())
}

#[test]
fn generated_ids_match_installed_php() {
    let Some(php) = php_binary() else {
        eprintln!("ids_generated: no `php` on PATH (set RPHP_PHP); skipping");
        return;
    };
    let out = Command::new(&php)
        .args(["-n", "-r", PHP_SCRIPT])
        .output()
        .expect("run php");
    assert!(
        out.status.success(),
        "php failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8(out.stdout).expect("utf-8 dump");

    let mut php_consts: BTreeMap<(u16, String), ()> = BTreeMap::new();
    let mut php_names: BTreeMap<u16, String> = BTreeMap::new();
    let mut version = String::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        match f.as_slice() {
            ["C", name, id] => {
                php_consts.insert((id.parse().unwrap(), name.to_string()), ());
            }
            ["N", id, name] => {
                php_names.insert(id.parse().unwrap(), name.to_string());
            }
            ["V", v] => version = v.to_string(),
            _ => panic!("unexpected dump line {line:?}"),
        }
    }
    assert!(
        !php_consts.is_empty(),
        "php reported no tokenizer constants"
    );

    let ours: BTreeMap<(u16, String), ()> = rphp_tokenizer::ids::ALL
        .iter()
        .map(|(n, id)| ((*id, n.to_string()), ()))
        .collect();
    let missing: Vec<_> = php_consts
        .keys()
        .filter(|k| !ours.contains_key(*k))
        .collect();
    let extra: Vec<_> = ours
        .keys()
        .filter(|k| !php_consts.contains_key(*k))
        .collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "src/ids.rs is stale for PHP {version}: missing {missing:?}, extra {extra:?} — run `cargo xtask token-ids`"
    );
    assert_eq!(
        rphp_tokenizer::ids::ALL.len(),
        php_consts.len(),
        "duplicate entries in ids::ALL"
    );
    assert!(
        rphp_tokenizer::ids::ALL
            .windows(2)
            .all(|w| (w[0].1, w[0].0) < (w[1].1, w[1].0)),
        "ids::ALL must be sorted by (id, name)"
    );

    for id in 256..=450u16 {
        assert_eq!(
            rphp_tokenizer::token_name(id).map(str::to_string),
            php_names.get(&id).cloned(),
            "token_name({id}) differs from PHP {version}"
        );
    }
    let min = rphp_tokenizer::ids::ALL.iter().map(|c| c.1).min().unwrap();
    let max = rphp_tokenizer::ids::ALL.iter().map(|c| c.1).max().unwrap();
    assert_eq!(
        (rphp_tokenizer::ids::MIN_ID, rphp_tokenizer::ids::MAX_ID),
        (min, max)
    );
}
