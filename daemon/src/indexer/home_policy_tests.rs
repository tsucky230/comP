//! HomePolicy and Roots::load_with: hidden folders of the home directory are
//! never additional roots unless the environment explicitly allows it.
//! Owned by the designer role; implementers must not edit.

use super::*;
use std::path::Path;
use tempfile::TempDir;

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

fn policy(homes: &[&str]) -> HomePolicy {
    HomePolicy::new(&s(homes), false)
}

#[test]
fn blocks_table_drive_home() {
    let p = policy(&[r"C:\Users\u"]);
    let cases: &[(&str, bool)] = &[
        // dot folders directly under the home: the folder itself and below
        (r"C:\Users\u\.ssh", true),
        (r"C:\Users\u\.ssh\id_rsa", true),
        ("c:/users/U/.SSH/id_rsa", true),
        (r"C:\Users\u\.config\nvim\init.lua", true),
        (r"C:\Users\u\.claude\.credentials.json", true),
        // AppData and Library, any case on a drive path
        (r"C:\Users\u\AppData", true),
        (r"C:\Users\u\AppData\Roaming\Code\User\globalStorage\state.vscdb", true),
        (r"C:\Users\u\appdata\local\Google\Chrome\User Data\Default", true),
        (r"C:\Users\u\AppData\LocalLow\x", true),
        (r"C:\Users\u\Library\Keychains", true),
        (r"C:\Users\u\LIBRARY", true),
        // the Windows temp directory is the one exception, in any case
        (r"C:\Users\u\AppData\Local\Temp", false),
        (r"C:\Users\u\AppData\Local\Temp\.tmpAbc123\lib.rs", false),
        (r"c:\users\u\appdata\local\temp\x", false),
        // ...and only that folder
        (r"C:\Users\u\AppData\Local", true),
        (r"C:\Users\u\AppData\Local\Temporary\x", true),
        (r"C:\Users\u\AppData\Roaming\Temp\x", true),
        // `..` and the verbatim prefix are resolved first
        (r"C:\Users\u\projects\..\.ssh\id_rsa", true),
        (r"C:\Users\u\AppData\Local\Temp\..\Microsoft\Credentials", true),
        (r"\\?\C:\Users\u\.ssh\id_rsa", true),
        (r"\\?\c:\users\u\appdata\local\temp\x", false),
        // not directly under the home
        (r"C:\Users\u\projects\.hidden\x", false),
        (r"C:\Users\u\projects\AppData\x", false),
        (r"C:\Users\u\projects\lib", false),
        (r"C:\Users\u\Documents", false),
        // the home itself, its parent, a sibling sharing the prefix, another drive
        (r"C:\Users\u", false),
        (r"C:\Users\.ssh", false),
        (r"C:\Users\u2\.ssh", false),
        (r"D:\Users\u\.ssh", false),
        // not absolute
        (".ssh/id_rsa", false),
        ("", false),
    ];
    for (input, want) in cases {
        assert_eq!(p.blocks(input), *want, "path: {:?}", input);
    }
}

#[test]
fn blocks_table_slash_home() {
    let p = policy(&["/home/u"]);
    let cases: &[(&str, bool)] = &[
        ("/home/u/.ssh/id_rsa", true),
        ("/home/u/.aws/credentials", true),
        ("/home/u/Library/Keychains/login.keychain-db", true),
        ("/home/u/AppData/x", true),
        ("/home/u/./.gnupg/../.ssh", true),
        // names compare exactly on slash paths
        ("/home/u/library/x", false),
        ("/home/u/appdata/x", false),
        ("/home/U/.ssh", false),
        // the temp exception is the same folder name
        ("/home/u/AppData/Local/Temp/x", false),
        // not directly under the home, the home, a prefix sibling, the parent
        ("/home/u/projects/.git/config", false),
        ("/home/u", false),
        ("/home/u2/.ssh", false),
        ("/home/u/..", false),
        ("/home/.ssh", false),
        // a drive path never matches a slash-path home
        ("C:/home/u/.ssh", false),
    ];
    for (input, want) in cases {
        assert_eq!(p.blocks(input), *want, "path: {:?}", input);
    }
}

#[test]
fn allow_turns_blocking_off() {
    let p = HomePolicy::new(&s(&[r"C:\Users\u"]), true);
    assert!(p.allows());
    for path in [r"C:\Users\u\.ssh\id_rsa", r"C:\Users\u\AppData\Roaming\x", r"C:\Users\u\Library"] {
        assert!(!p.blocks(path), "path: {}", path);
    }
}

#[test]
fn new_normalizes_and_deduplicates_homes() {
    let p = HomePolicy::new(&s(&[r"c:\Users\u\", "C:/users/U", "relative/home", "", "/home/u", "/home/u/"]), false);
    assert_eq!(p.homes(), &s(&["C:/Users/u", "/home/u"])[..]);
    assert!(!p.allows());
    assert!(!HomePolicy::new(&[], false).blocks(r"C:\Users\u\.ssh"));
}

#[test]
fn several_homes_are_all_protected() {
    let p = policy(&["/home/u", r"C:\Users\u"]);
    assert!(p.blocks("/home/u/.ssh"));
    assert!(p.blocks(r"C:\Users\u\AppData"));
    assert!(!p.blocks("/home/v/.ssh"));
}

#[test]
fn from_vars_reads_the_allow_switch() {
    let cases: &[(Option<&str>, bool)] = &[
        (None, false),
        (Some(""), false),
        (Some("0"), false),
        (Some("false"), false),
        (Some("no"), false),
        (Some("yes"), false),
        (Some("on"), false),
        (Some("2"), false),
        (Some("1"), true),
        (Some("true"), true),
        (Some("TRUE"), true),
        (Some(" True "), true),
        (Some("1\n"), true),
    ];
    for (raw, want) in cases {
        let p = HomePolicy::from_vars(Some("/home/u"), None, *raw);
        assert_eq!(p.allows(), *want, "allow: {:?}", raw);
        assert_eq!(p.blocks("/home/u/.ssh"), !*want, "allow: {:?}", raw);
    }
}

#[test]
fn from_vars_keeps_a_missing_home_and_uses_both_variables() {
    let tmp = TempDir::new().unwrap();
    let missing = tmp.path().join("no-such-home");
    let missing_str = missing.to_string_lossy().to_string();

    let p = HomePolicy::from_vars(Some(&missing_str), Some("/home/other-user-xyz"), None);

    let want = vec![normalize_abs(&missing_str).unwrap(), "/home/other-user-xyz".to_string()];
    assert_eq!(p.homes(), &want[..]);
    assert!(p.blocks(&format!("{}/.ssh/id_rsa", missing_str)));
    assert!(p.blocks("/home/other-user-xyz/.aws"));

    assert!(HomePolicy::from_vars(None, None, None).homes().is_empty());
    assert!(HomePolicy::from_vars(Some("relative"), Some(""), None).homes().is_empty());
}

#[cfg(unix)]
#[test]
fn from_vars_adds_the_canonical_form_of_a_symlinked_home() {
    let tmp = TempDir::new().unwrap();
    let real = tmp.path().join("real-home");
    std::fs::create_dir_all(&real).unwrap();
    let link = tmp.path().join("link-home");
    std::os::unix::fs::symlink(&real, &link).unwrap();

    let p = HomePolicy::from_vars(Some(&link.to_string_lossy()), None, None);

    let canonical = real.canonicalize().unwrap();
    assert!(p.blocks(&format!("{}/.ssh/id_rsa", link.to_string_lossy())));
    assert!(p.blocks(&format!("{}/.ssh/id_rsa", canonical.to_string_lossy())));
}

// ---------------------------------------------------------------- load_with

fn mkdir(base: &Path, rel: &str) -> String {
    let p = base.join(rel);
    std::fs::create_dir_all(&p).unwrap();
    p.to_string_lossy().to_string()
}

fn set_roots(main: &Path, entries: &[String]) {
    std::fs::create_dir_all(main.join(".comp")).unwrap();
    let config = serde_json::json!({ "additional_paths": entries });
    std::fs::write(main.join(".comp/config.json"), config.to_string()).unwrap();
}

fn norm(p: &str) -> String {
    normalize_abs(p).unwrap()
}

#[test]
fn load_with_drops_hidden_home_folders_and_keeps_the_rest() {
    let (home, main) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let h = home.path();
    let home_name = h.file_name().unwrap().to_string_lossy().to_string();
    let ssh = mkdir(h, ".ssh");
    let nvim = mkdir(h, ".config/nvim");
    let roaming = mkdir(h, "AppData/Roaming");
    let library = mkdir(h, "Library");
    let lib = mkdir(h, "projects/lib");
    let temp_work = mkdir(h, "AppData/Local/Temp/work");
    // `../<home>/.ssh` from the main root is the relative form of the same attack
    let relative_ssh = format!("../{}/.ssh", home_name);
    set_roots(
        main.path(),
        &[relative_ssh, nvim.clone(), roaming.clone(), library.clone(), lib.clone(), temp_work.clone()],
    );
    let main_str = main.path().to_string_lossy().to_string();

    let blocked = Roots::load_with(&main_str, &HomePolicy::new(&[h.to_string_lossy().to_string()], false));
    assert_eq!(blocked.extras(), &[norm(&lib), norm(&temp_work)][..]);
    assert_eq!(blocked.main(), norm(&main_str));

    let allowed = Roots::load_with(&main_str, &HomePolicy::new(&[h.to_string_lossy().to_string()], true));
    let want: Vec<String> = [&ssh, &nvim, &roaming, &library, &lib, &temp_work].iter().map(|p| norm(p)).collect();
    assert_eq!(allowed.extras(), &want[..]);
}

#[test]
fn load_with_never_judges_the_main_root() {
    let home = TempDir::new().unwrap();
    let main = mkdir(home.path(), ".config/app");
    let lib = mkdir(home.path(), "projects/lib");
    set_roots(Path::new(&main), &[lib.clone()]);

    let roots = Roots::load_with(&main, &HomePolicy::new(&[home.path().to_string_lossy().to_string()], false));

    assert_eq!(roots.main(), norm(&main));
    assert_eq!(roots.extras(), &[norm(&lib)][..]);
}

#[cfg(unix)]
#[test]
fn load_with_refuses_a_symlink_into_a_hidden_home_folder() {
    let (home, main) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let ssh = mkdir(home.path(), ".ssh");
    mkdir(home.path(), "projects");
    let innocent = home.path().join("projects/innocent");
    std::os::unix::fs::symlink(&ssh, &innocent).unwrap();
    set_roots(main.path(), &[innocent.to_string_lossy().to_string()]);

    // WHY from_vars, as from_env does: on macOS the temp folder sits behind a
    // symlink (/var -> /private/var), so the link's canonical target is only
    // under the home once the home's canonical form is known too.
    let roots = Roots::load_with(
        &main.path().to_string_lossy(),
        &HomePolicy::from_vars(Some(&home.path().to_string_lossy()), None, None),
    );

    assert!(roots.extras().is_empty(), "kept: {:?}", roots.extras());
}

/// `Roots::load` (indexing, MCP) applies the protection taken from the
/// process environment. Uses a real hidden folder of the real home; nothing
/// in it is opened or read.
#[test]
fn load_applies_the_environment_policy() {
    let env = HomePolicy::from_env();
    let hidden = env
        .homes()
        .iter()
        .find_map(|home| {
            let mut names: Vec<std::path::PathBuf> = std::fs::read_dir(home)
                .ok()?
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                .filter(|e| e.file_name().to_string_lossy().starts_with('.'))
                .map(|e| e.path())
                .collect();
            names.sort();
            names.into_iter().next()
        })
        .expect("no hidden folder under $HOME / $USERPROFILE to test with");
    let main = TempDir::new().unwrap();
    set_roots(main.path(), &[hidden.to_string_lossy().to_string()]);

    let roots = Roots::load(&main.path().to_string_lossy());

    if env.allows() {
        assert_eq!(roots.extras(), &[norm(&hidden.to_string_lossy())][..]);
    } else {
        assert!(roots.extras().is_empty(), "{} must be refused: {:?}", hidden.display(), roots.extras());
    }
}

/// An additional root as wide as the home is walked, but nothing in its
/// protected folders enters the index, and what an older index holds there
/// is removed. The walker skips dot folders by itself; `Library` and
/// `AppData` are not dot folders, so this pins the extra filter.
#[tokio::test]
async fn indexing_a_root_as_wide_as_the_home_skips_its_protected_folders() {
    let (home, main) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let w = |dir: &Path, rel: &str, body: &str| {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    };
    w(home.path(), "projects/a.rs", "fn ordinary_fn() {}
");
    w(home.path(), "Library/Application Support/app/settings.json", "{\"token\": \"x\"}
");
    w(home.path(), "AppData/Roaming/app/config.json", "{\"secret\": \"y\"}
");
    w(home.path(), "AppData/Local/Temp/work/t.rs", "fn temp_fn() {}
");
    w(main.path(), "m.rs", "fn main_fn() {}
");
    set_roots(main.path(), &[home.path().to_string_lossy().to_string()]);
    let main_str = main.path().to_string_lossy().to_string();
    let home_str = home.path().to_string_lossy().to_string();
    let db = crate::graph::GraphDB::new(&main_str).await.unwrap();
    let stored = |db: &crate::graph::GraphDB| {
        let mut v: Vec<String> = db.list_files().unwrap().into_iter().map(|(_, p, _)| p).collect();
        v.sort();
        v
    };
    let h = norm(&home_str);

    // An index built while the protection was off holds the secrets.
    crate::indexer::index_all_roots_with(&main_str, &db, &HomePolicy::new(&[home_str.clone()], true)).await.unwrap();
    assert!(stored(&db).contains(&format!("{}/Library/Application Support/app/settings.json", h)));
    assert!(stored(&db).contains(&format!("{}/AppData/Roaming/app/config.json", h)));

    crate::indexer::index_all_roots_with(&main_str, &db, &HomePolicy::new(&[home_str.clone()], false)).await.unwrap();

    let mut want = vec![
        "m.rs".to_string(),
        format!("{}/projects/a.rs", h),
        format!("{}/AppData/Local/Temp/work/t.rs", h),
    ];
    want.sort();
    assert_eq!(stored(&db), want);
}

mod properties {
    use super::*;
    use proptest::prelude::*;

    fn segment() -> impl Strategy<Value = String> {
        prop_oneof![
            Just(".ssh".to_string()),
            Just(".config".to_string()),
            Just("AppData".to_string()),
            Just("appdata".to_string()),
            Just("Local".to_string()),
            Just("LOCAL".to_string()),
            Just("Temp".to_string()),
            Just("temp".to_string()),
            Just("Roaming".to_string()),
            Just("Library".to_string()),
            Just("library".to_string()),
            Just("projects".to_string()),
            "[a-zA-Z0-9_]{1,6}",
            "\\.[a-z]{1,4}",
        ]
    }

    /// The specification written out independently of the implementation:
    /// protected first segment, minus the AppData/Local/Temp subtree.
    fn expected(drive: bool, segs: &[String]) -> bool {
        let eq = |a: &str, b: &str| if drive { a.eq_ignore_ascii_case(b) } else { a == b };
        let first = &segs[0];
        let protected = first.starts_with('.') || eq(first, "AppData") || eq(first, "Library");
        let temp = segs.len() >= 3 && eq(&segs[0], "AppData") && eq(&segs[1], "Local") && eq(&segs[2], "Temp");
        protected && !temp
    }

    proptest! {
        /// Every spelling of a path under the home gets the specified answer.
        #[test]
        fn blocks_matches_the_specification_in_any_spelling(
            drive in any::<bool>(),
            segs in prop::collection::vec(segment(), 1..5),
            backslashes in any::<bool>(),
            flip_home_case in any::<bool>(),
            verbatim in any::<bool>(),
        ) {
            let home = if drive { "C:/Users/u" } else { "/home/u" };
            let p = HomePolicy::new(&[home.to_string()], false);
            let want = expected(drive, &segs);

            let mut spelled_home = home.to_string();
            if drive && flip_home_case {
                spelled_home = spelled_home.to_ascii_lowercase();
            }
            let mut path = format!("{}/{}", spelled_home, segs.join("/"));
            if drive && backslashes {
                path = path.replace('/', "\\");
            }
            if drive && verbatim {
                path = format!(r"\\?\{}", path.replace('/', "\\"));
            }

            prop_assert_eq!(p.blocks(&path), want, "path: {}", path);
            prop_assert!(!HomePolicy::new(&[home.to_string()], true).blocks(&path));
        }
    }
}
