use super::*;

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

#[test]
fn normalize_abs_table() {
    let cases: &[(&str, Option<&str>)] = &[
        // drive paths: separators, drive-letter case, trailing separator
        ("E:\\dev\\app", Some("E:/dev/app")),
        ("e:\\dev\\app", Some("E:/dev/app")),
        ("e:/dev/app/", Some("E:/dev/app")),
        ("E:\\dev\\\\app\\.\\src", Some("E:/dev/app/src")),
        ("E:/dev/app/../lib", Some("E:/dev/lib")),
        ("E:/", Some("E:/")),
        ("e:\\", Some("E:/")),
        // only the drive letter is re-cased; segments keep their spelling
        ("e:/Dev/App", Some("E:/Dev/App")),
        // slash paths
        ("/home/u/app", Some("/home/u/app")),
        ("/home/u/app/", Some("/home/u/app")),
        ("/home/u/./app/../lib", Some("/home/u/lib")),
        ("/", Some("/")),
        ("//server/share/dir", Some("//server/share/dir")),
        ("\\\\server\\share\\dir", Some("//server/share/dir")),
        // verbatim prefix from std::fs::canonicalize
        (r"\\?\E:\dev\app", Some("E:/dev/app")),
        (r"\\?\e:\dev\app\src\a.rs", Some("E:/dev/app/src/a.rs")),
        ("//?/E:/dev/app", Some("E:/dev/app")),
        (r"\\?\UNC\server\share\dir", Some("//server/share/dir")),
        (r"\\?\unc\server\share", Some("//server/share")),
        (r"\\?\Volume{b75e2c83-0000-0000-0000-602f00000000}\x", None),
        (r"\\?\", None),
        // climbing above the root
        ("E:/..", None),
        ("E:/dev/../../x", None),
        ("/..", None),
        // not absolute
        ("", None),
        ("src/a.rs", None),
        ("../lib", None),
        ("E:", None),
        ("E:dev", None),
        (".", None),
    ];
    for (input, want) in cases {
        assert_eq!(normalize_abs(input).as_deref(), *want, "input: {:?}", input);
    }
}

#[test]
fn new_resolves_relative_entries_against_main() {
    let r = Roots::new("E:\\dev\\app", &s(&["../shared-lib", "..\\other\\", "/abs/else"]));
    assert_eq!(r.main(), "E:/dev/app");
    assert_eq!(r.extras(), &s(&["E:/dev/shared-lib", "E:/dev/other", "/abs/else"])[..]);
}

#[test]
fn new_drops_unusable_entries_and_keeps_order() {
    let cases: &[(&[&str], &[&str])] = &[
        // the main root itself, in three spellings
        (&["E:/dev/app", "e:\\dev\\app\\", "."], &[]),
        // inside the main root
        (&["E:/dev/app/packages/x", "packages/y"], &[]),
        // ancestors of the main root
        (&["E:/dev", "E:/", ".."], &[]),
        // a sibling whose name merely starts with the main root's name is kept
        (&["E:/dev/app2"], &["E:/dev/app2"]),
        // duplicates: first spelling wins, drive paths compare case-insensitively
        (&["E:/dev/lib", "e:\\DEV\\LIB", "../lib"], &["E:/dev/lib"]),
        // an entry inside another kept entry is dropped, whichever comes first
        (&["E:/dev/mono/pkg", "E:/dev/mono"], &["E:/dev/mono"]),
        (&["E:/dev/mono", "E:/dev/mono/pkg"], &["E:/dev/mono"]),
        // empty and un-normalizable entries
        (&["", "../../../..", "E:/dev/lib"], &["E:/dev/lib"]),
        // order of survivors is preserved
        (&["E:/z", "E:/dev/app/in", "E:/a"], &["E:/z", "E:/a"]),
    ];
    for (entries, want) in cases {
        let r = Roots::new("E:/dev/app", &s(entries));
        assert_eq!(r.extras(), &s(want)[..], "entries: {:?}", entries);
    }
}

#[test]
fn new_slash_paths_are_case_sensitive() {
    let r = Roots::new("/home/u/app", &s(&["/home/u/Lib", "/home/u/lib", "/home/U/app"]));
    assert_eq!(r.extras(), &s(&["/home/u/Lib", "/home/u/lib", "/home/U/app"])[..]);
}

#[test]
fn new_with_non_absolute_main_is_single_root() {
    let r = Roots::new(".", &s(&["E:/dev/lib", "../lib"]));
    assert_eq!(r.main(), ".");
    assert!(r.extras().is_empty());
}

fn two_roots() -> Roots {
    Roots::new("E:/dev/app", &s(&["E:/dev/lib", "/srv/shared"]))
}

#[test]
fn locate_table() {
    let main = |p: &str| Some(Located { owner: Owner::Main, stored_path: p.to_string() });
    let extra = |i: usize, p: &str| Some(Located { owner: Owner::Extra(i), stored_path: p.to_string() });
    let cases: Vec<(&str, Option<Located>)> = vec![
        // main root: stored relative, whatever the spelling of the root part
        ("E:/dev/app/src/a.rs", main("src/a.rs")),
        ("e:\\dev\\app\\src\\a.rs", main("src/a.rs")),
        ("E:\\DEV\\APP\\Src\\A.rs", main("Src/A.rs")),
        ("E:/dev/app/README.md", main("README.md")),
        // additional roots: stored absolute, root part canonical, rest as given
        ("E:/dev/lib/src/a.rs", extra(0, "E:/dev/lib/src/a.rs")),
        ("e:\\DEV\\LIB\\Src\\A.rs", extra(0, "E:/dev/lib/Src/A.rs")),
        ("/srv/shared/README.md", extra(1, "/srv/shared/README.md")),
        // whole-segment containment
        ("E:/dev/app2/x.rs", None),
        ("E:/dev/library/x.rs", None),
        ("/srv/shared2/x.rs", None),
        // slash roots are case-sensitive
        ("/srv/Shared/x.rs", None),
        // `..` is resolved before the containment check
        ("E:/dev/app/../secret/x.rs", None),
        ("E:/dev/lib/../../x.rs", None),
        ("E:/dev/app/../lib/x.rs", extra(0, "E:/dev/lib/x.rs")),
        ("E:/dev/app/src/../a.rs", main("a.rs")),
        // outside every root, a root itself, not absolute
        ("C:/Windows/system.ini", None),
        ("E:/dev", None),
        ("E:/dev/app", None),
        ("E:/dev/lib/", None),
        ("src/a.rs", None),
        ("", None),
    ];
    for (input, want) in cases {
        assert_eq!(two_roots().locate(input), want, "input: {:?}", input);
    }
}

#[test]
fn owner_of_stored_table() {
    let cases: &[(&str, Option<Owner>)] = &[
        ("src/a.rs", Some(Owner::Main)),
        (".comp/history/log-2026-11.jsonl", Some(Owner::Main)),
        ("E:/dev/lib/src/a.rs", Some(Owner::Extra(0))),
        ("e:/DEV/lib/src/a.rs", Some(Owner::Extra(0))),
        ("/srv/shared/x.md", Some(Owner::Extra(1))),
        // absolute paths of roots that are no longer configured
        ("E:/dev/removed/x.rs", None),
        ("E:/dev/library/x.rs", None),
        ("/srv/Shared/x.md", None),
    ];
    for (input, want) in cases {
        assert_eq!(two_roots().owner_of_stored(input), *want, "input: {:?}", input);
    }
}

#[test]
fn stored_to_abs_table() {
    let cases: &[(&str, Option<&str>)] = &[
        ("src/a.rs", Some("E:/dev/app/src/a.rs")),
        ("README.md", Some("E:/dev/app/README.md")),
        ("E:/dev/lib/src/a.rs", Some("E:/dev/lib/src/a.rs")),
        ("/srv/shared/x.md", Some("/srv/shared/x.md")),
        ("E:/dev/removed/x.rs", None),
        ("../outside.rs", None),
        ("src/../../outside.rs", None),
    ];
    for (input, want) in cases {
        assert_eq!(two_roots().stored_to_abs(input).as_deref(), *want, "input: {:?}", input);
    }
}

#[test]
fn locate_accepts_the_verbatim_form() {
    let r = two_roots();
    assert_eq!(
        r.locate(r"\\?\E:\dev\app\src\a.rs"),
        Some(Located { owner: Owner::Main, stored_path: "src/a.rs".to_string() })
    );
    assert_eq!(
        r.locate(r"\\?\e:\DEV\lib\src\a.rs"),
        Some(Located { owner: Owner::Extra(0), stored_path: "E:/dev/lib/src/a.rs".to_string() })
    );
}

#[test]
fn is_current_stored_table() {
    let cases: &[(&str, bool)] = &[
        ("src/a.rs", true),
        (".comp/history/log-2026-11.jsonl", true),
        ("E:/dev/lib/src/a.rs", true),
        ("E:/dev/lib/Src/A.rs", true),
        ("/srv/shared/x.md", true),
        // another spelling of a current root
        ("E:/DEV/lib/src/a.rs", false),
        ("e:/dev/lib/src/a.rs", false),
        (r"E:\dev\lib\src\a.rs", false),
        // a root that is no longer configured
        ("E:/dev/removed/x.rs", false),
        ("/srv/Shared/x.md", false),
        // absolute aliases of a main-root file
        ("E:/dev/app/src/a.rs", false),
        ("//?/E:/dev/app/src/a.rs", false),
        ("//?/E:/dev/lib/src/a.rs", false),
    ];
    for (input, want) in cases {
        assert_eq!(two_roots().is_current_stored(input), *want, "input: {:?}", input);
    }
}

mod properties {
    use super::*;
    use proptest::prelude::*;

    /// One path segment: letters of both cases, digits, and the characters
    /// that most often trip prefix matching.
    fn segment() -> impl Strategy<Value = String> {
        "[a-zA-Z0-9_. -]{1,8}".prop_filter("not a dot segment", |s| s != "." && s != "..")
    }

    fn rel_path() -> impl Strategy<Value = Vec<String>> {
        prop::collection::vec(segment(), 1..5)
    }

    /// A root in either style, already normalized.
    fn root() -> impl Strategy<Value = String> {
        (any::<bool>(), "[A-Z]", prop::collection::vec(segment(), 1..4)).prop_map(|(drive, letter, segs)| {
            if drive {
                format!("{}:/{}", letter, segs.join("/"))
            } else {
                format!("/{}", segs.join("/"))
            }
        })
    }

    /// Random re-spelling that must not change which file is meant: separator
    /// style everywhere, and letter case for drive paths.
    fn respell(path: &str, backslashes: bool, upper: bool) -> String {
        let mut s = path.to_string();
        if s.as_bytes().get(1) == Some(&b':') {
            s = if upper { s.to_ascii_uppercase() } else { s.to_ascii_lowercase() };
        }
        if backslashes {
            s = s.replace('/', "\\");
        }
        s
    }

    proptest! {
        /// Whatever is located lies inside the root that claims it, and the
        /// stored path maps back to the same file.
        #[test]
        fn located_paths_are_inside_their_root(
            main in root(),
            extras in prop::collection::vec(root(), 0..3),
            unrelated in root(),
            pick in any::<prop::sample::Index>(),
            rel in rel_path(),
        ) {
            let roots = Roots::new(&main, &extras);
            // Mostly a real root, sometimes a directory in no root at all.
            let mut bases = vec![roots.main().to_string(), unrelated];
            bases.extend(roots.extras().iter().cloned());
            let candidate = format!("{}/{}", pick.get(&bases), rel.join("/"));
            if let Some(found) = roots.locate(&candidate) {
                let owner_root = match found.owner {
                    Owner::Main => roots.main().to_string(),
                    Owner::Extra(i) => roots.extras()[i].clone(),
                };
                let abs = roots.stored_to_abs(&found.stored_path).expect("located path maps back");
                let normalized = normalize_abs(&candidate).unwrap();
                let fold = |s: &str| if s.as_bytes().get(1) == Some(&b':') { s.to_ascii_lowercase() } else { s.to_string() };
                prop_assert_eq!(fold(&abs), fold(&normalized));
                // A bare drive root (`A:/`) already ends with the separator.
                let owner_dir = format!("{}/", fold(&owner_root).trim_end_matches('/'));
                prop_assert!(fold(&abs).starts_with(&owner_dir), "outside its root: {}", abs);
                prop_assert_eq!(roots.owner_of_stored(&found.stored_path), Some(found.owner.clone()));
                prop_assert!(roots.is_current_stored(&found.stored_path));
            }
        }

        /// A file under a root is found under that root however the path is
        /// spelled, and every spelling yields one stored path.
        #[test]
        fn every_spelling_of_a_file_has_one_stored_path(
            main in root(),
            extras in prop::collection::vec(root(), 0..3),
            pick in any::<prop::sample::Index>(),
            rel in rel_path(),
            backslashes in any::<bool>(),
            upper in any::<bool>(),
        ) {
            let roots = Roots::new(&main, &extras);
            let mut all = vec![roots.main().to_string()];
            all.extend(roots.extras().iter().cloned());
            let chosen = pick.get(&all).clone();
            let plain = format!("{}/{}", chosen, rel.join("/"));

            let canonical = roots.locate(&plain);
            prop_assert!(canonical.is_some(), "a file under a root must be located: {}", plain);
            let respelled_root = respell(&chosen, backslashes, upper);
            let sep = if backslashes { "\\" } else { "/" };
            let respelled = format!("{}{}{}", respelled_root, sep, rel.join(sep));
            prop_assert_eq!(roots.locate(&respelled), canonical);
        }

        /// No sequence of `..` gets a path out of the roots: the result is
        /// either rejected or genuinely inside one.
        #[test]
        fn dot_dot_never_escapes(
            main in root(),
            extras in prop::collection::vec(root(), 0..3),
            parts in prop::collection::vec(prop_oneof![segment(), Just("..".to_string())], 1..8),
        ) {
            let roots = Roots::new(&main, &extras);
            let candidate = format!("{}/{}", roots.main(), parts.join("/"));
            match (roots.locate(&candidate), normalize_abs(&candidate)) {
                (Some(_), None) => prop_assert!(false, "located a path that cannot be normalized"),
                (Some(found), Some(real)) => {
                    let abs = roots.stored_to_abs(&found.stored_path).expect("maps back");
                    let fold = |s: &str| if s.as_bytes().get(1) == Some(&b':') { s.to_ascii_lowercase() } else { s.to_string() };
                    prop_assert_eq!(fold(&abs), fold(&real));
                }
                (None, _) => {}
            }
        }
    }
}

/// locate → stored_to_abs → locate returns the same answer, for both owners.
#[test]
fn locate_and_stored_to_abs_round_trip() {
    let r = two_roots();
    for input in ["e:\\dev\\app\\src\\a.rs", "e:\\DEV\\LIB\\Src\\A.rs", "/srv/shared/d/x.md"] {
        let first = r.locate(input).expect("inside a root");
        let abs = r.stored_to_abs(&first.stored_path).expect("owned stored path");
        assert_eq!(r.locate(&abs), Some(first.clone()), "input: {:?}", input);
        assert_eq!(r.owner_of_stored(&first.stored_path), Some(first.owner), "input: {:?}", input);
    }
}

#[test]
fn load_reads_both_keys_and_drops_missing_directories() {
    use tempfile::TempDir;

    let main = TempDir::new().unwrap();
    let by_hand = TempDir::new().unwrap();
    let by_extension = TempDir::new().unwrap();
    let missing = by_hand.path().join("does-not-exist");
    let a_file = by_hand.path().join("file.txt");
    std::fs::write(&a_file, "x").unwrap();

    std::fs::create_dir_all(main.path().join(".comp")).unwrap();
    let config = serde_json::json!({
        "exclude": ["dist"],
        "additional_paths": [by_hand.path().to_string_lossy(), missing.to_string_lossy(), 42],
        "workspace_folders": [by_extension.path().to_string_lossy(), a_file.to_string_lossy()],
    });
    std::fs::write(main.path().join(".comp/config.json"), config.to_string()).unwrap();

    let r = Roots::load(&main.path().to_string_lossy());
    let want = vec![
        normalize_abs(&by_hand.path().to_string_lossy()).unwrap(),
        normalize_abs(&by_extension.path().to_string_lossy()).unwrap(),
    ];
    assert_eq!(r.main(), normalize_abs(&main.path().to_string_lossy()).unwrap());
    assert_eq!(r.extras(), &want[..]);
}

#[test]
fn load_without_usable_config_is_single_root() {
    use tempfile::TempDir;

    for body in [None, Some("{ not json"), Some("[]"), Some(r#"{"additional_paths": "E:/dev/lib"}"#)] {
        let main = TempDir::new().unwrap();
        if let Some(text) = body {
            std::fs::create_dir_all(main.path().join(".comp")).unwrap();
            std::fs::write(main.path().join(".comp/config.json"), text).unwrap();
        }
        let r = Roots::load(&main.path().to_string_lossy());
        assert_eq!(r.main(), normalize_abs(&main.path().to_string_lossy()).unwrap(), "config: {:?}", body);
        assert!(r.extras().is_empty(), "config: {:?}", body);
    }
}
