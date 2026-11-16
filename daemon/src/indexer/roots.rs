//! Which directories one index covers, and how a file's path is stored in it.
//!
//! One `.comp/index.db` can cover the main workspace root plus additional
//! roots (`additional_paths` / `workspace_folders` in `.comp/config.json`).
//!
//! Stored-path rule (the `files.path` column):
//! - a file under the main root is stored **relative** to it (`src/a.rs`) —
//!   unchanged from single-root indexes, so existing databases stay valid;
//! - a file under an additional root is stored as its **absolute** path
//!   (`E:/dev/lib/src/a.rs`), so same-named files in different roots never
//!   collide and `Path::new(main_root).join(stored)` still resolves to it.
//!
//! Additional roots are refused when they lie in a hidden folder of the
//! user's home directory (`~/.ssh`, `~/AppData`, …) — see [`HomePolicy`].
//!
//! Everything here except [`Roots::load`], [`Roots::load_with`] and
//! [`HomePolicy::from_vars`] / [`HomePolicy::from_env`] is lexical: no
//! filesystem access, no canonicalization. Both path styles are understood on every platform so
//! behaviour does not depend on where the tests run:
//! - drive paths (`E:\dev\app`, `e:/dev/app`) — compared case-insensitively;
//! - slash paths (`/home/u/app`, `//server/share`) — compared case-sensitively.

/// The root a path belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Owner {
    Main,
    /// Index into [`Roots::extras`].
    Extra(usize),
}

/// Result of [`Roots::locate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Located {
    pub owner: Owner,
    /// Value for the `files.path` column (see the module doc's stored-path rule).
    pub stored_path: String,
}

/// Main root plus additional roots, all normalized by [`normalize_abs`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Roots {
    main: String,
    extras: Vec<String>,
}

/// Name of the environment variable that turns the hidden-home-folder
/// protection off. Only the process environment can do so: a repository
/// controls `.comp/config.json`, so a switch there would defeat the purpose.
pub const ALLOW_HIDDEN_HOME_ROOTS_ENV: &str = "COMP_ALLOW_HIDDEN_HOME_ROOTS";

/// Which paths count as "inside a hidden folder of the home directory", and
/// whether additional roots may reach them.
///
/// Threat: a repository's `.comp/config.json` can name any directory as an
/// additional root (`"additional_paths": ["../../.ssh"]`), and files under an
/// additional root become readable through MCP tools such as `compress_file`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomePolicy {
    homes: Vec<String>,
    allow: bool,
}

impl HomePolicy {
    /// `homes` in any spelling [`normalize_abs`] accepts. Entries it rejects
    /// are dropped; duplicates (compared like the rest of this module) keep
    /// the first spelling. With `allow`, [`HomePolicy::blocks`] is always false.
    pub fn new(homes: &[String], allow: bool) -> HomePolicy {
        let mut kept: Vec<String> = Vec::new();
        for home in homes {
            let Some(h) = normalize_abs(home) else { continue };
            if !kept.iter().any(|k| same_path(k, &h)) {
                kept.push(h);
            }
        }
        HomePolicy { homes: kept, allow }
    }

    /// Build from raw environment values: `home` (`$HOME`), `userprofile`
    /// (`$USERPROFILE`) and `allow` (`$COMP_ALLOW_HIDDEN_HOME_ROOTS`).
    ///
    /// - Each home is used as given and, when it can be canonicalized, also
    ///   in its canonical form (a symlinked home). A home that does not exist
    ///   is kept as given; this never fails or panics.
    /// - `allow` is true only for `1` or `true`, ignoring ASCII case and
    ///   surrounding whitespace. Anything else, or `None`, keeps the
    ///   protection on.
    pub fn from_vars(home: Option<&str>, userprofile: Option<&str>, allow: Option<&str>) -> HomePolicy {
        let mut homes: Vec<String> = Vec::new();
        for raw in [home, userprofile].into_iter().flatten() {
            homes.push(raw.to_string());
            // Canonicalizing a relative value would resolve it against the
            // current directory and invent a home.
            if normalize_abs(raw).is_some() {
                if let Ok(canonical) = std::path::Path::new(raw).canonicalize() {
                    homes.push(canonical.to_string_lossy().to_string());
                }
            }
        }
        let allow = allow.is_some_and(|v| {
            let v = v.trim();
            v == "1" || v.eq_ignore_ascii_case("true")
        });
        HomePolicy::new(&homes, allow)
    }

    /// [`HomePolicy::from_vars`] with the process environment.
    pub fn from_env() -> HomePolicy {
        HomePolicy::from_vars(
            std::env::var("HOME").ok().as_deref(),
            std::env::var("USERPROFILE").ok().as_deref(),
            std::env::var(ALLOW_HIDDEN_HOME_ROOTS_ENV).ok().as_deref(),
        )
    }

    /// Normalized homes, in the order given.
    pub fn homes(&self) -> &[String] {
        &self.homes
    }

    pub fn allows(&self) -> bool {
        self.allow
    }

    /// True when the protection is on and `path` is a protected folder
    /// directly under one of the homes, or lies inside one.
    ///
    /// - Protected names: anything starting with `.`, `AppData` (Windows) and
    ///   `Library` (macOS). For a drive-path home the names compare ignoring
    ///   ASCII case (`appdata`), for a slash-path home they compare exactly.
    /// - Exception: `<home>/AppData/Local/Temp` and everything below it, the
    ///   Windows temp directory.
    /// - Only the first segment below a home counts: `~/src/.hidden` is not
    ///   protected, and neither is the home itself.
    /// - `path` may be in any spelling [`normalize_abs`] accepts, including
    ///   the `\\?\` form; a path it rejects is never blocked.
    pub fn blocks(&self, path: &str) -> bool {
        if self.allow {
            return false;
        }
        let Some(p) = normalize_abs(path) else { return false };
        self.homes.iter().any(|home| {
            if !is_inside(home, &p) {
                return false;
            }
            let eq = |a: &str, b: &str| if is_drive_path(home) { a.eq_ignore_ascii_case(b) } else { a == b };
            let segs: Vec<&str> = remainder(home, &p).split('/').collect();
            let first = segs[0];
            let protected = first.starts_with('.') || eq(first, "AppData") || eq(first, "Library");
            let temp = segs.len() >= 3 && eq(segs[0], "AppData") && eq(segs[1], "Local") && eq(segs[2], "Temp");
            protected && !temp
        })
    }
}

/// Lexically normalize an absolute path.
///
/// - `\` becomes `/`; repeated separators collapse (a leading `//` of a UNC
///   path is kept); `.` segments are dropped; `..` removes the previous segment.
/// - A drive letter is upper-cased (`e:\dev` → `E:/dev`).
/// - A Windows verbatim prefix, which `std::fs::canonicalize` adds, is removed:
///   `\\?\E:\dev` → `E:/dev`, `\\?\UNC\server\share\d` → `//server/share/d`
///   (`UNC` in any case). Any other `\\?\…` form is not absolute here.
/// - No trailing `/`, except for a bare root (`/`, `E:/`).
///
/// Returns `None` when `path` is empty, is not absolute (does not start with
/// `/`, `\` or `<letter>:` followed by a separator), or when `..` would climb
/// above the root.
pub fn normalize_abs(path: &str) -> Option<String> {
    let s0 = path.replace('\\', "/");
    let s: String = match s0.strip_prefix("//?/") {
        Some(rest) => {
            let rb = rest.as_bytes();
            if rb.len() >= 3 && rb[0].is_ascii_alphabetic() && rb[1] == b':' && rb[2] == b'/' {
                rest.to_string()
            } else if rb.len() >= 4 && rb[..3].eq_ignore_ascii_case(b"unc") && rb[3] == b'/' && !rest[4..].starts_with('/') {
                format!("//{}", &rest[4..])
            } else {
                return None;
            }
        }
        None => s0,
    };
    let b = s.as_bytes();
    let (prefix, rest): (String, &str) = if b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'/' {
        (format!("{}:/", (b[0] as char).to_ascii_uppercase()), &s[3..])
    } else if let Some(r) = s.strip_prefix("//").filter(|r| !r.starts_with('/')) {
        ("//".to_string(), r)
    } else {
        ("/".to_string(), s.strip_prefix('/')?)
    };
    let mut segments: Vec<&str> = Vec::new();
    for seg in rest.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                segments.pop()?;
            }
            other => segments.push(other),
        }
    }
    if segments.is_empty() && prefix == "//" {
        return None;
    }
    Some(format!("{}{}", prefix, segments.join("/")))
}

fn is_drive_path(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

/// Absolute in the strict sense used for stored paths: a leading separator, or
/// `<letter>:` followed by a separator.
pub(crate) fn looks_absolute(s: &str) -> bool {
    let b = s.as_bytes();
    s.starts_with('/')
        || s.starts_with('\\')
        || (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'/' || b[2] == b'\\'))
}

/// Comparison key: drive paths ignore ASCII case, slash paths do not.
fn key(s: &str) -> String {
    if is_drive_path(s) {
        s.to_ascii_lowercase()
    } else {
        s.to_string()
    }
}

fn dir_prefix(root: &str) -> String {
    if root.ends_with('/') {
        root.to_string()
    } else {
        format!("{}/", root)
    }
}

/// `path` lies strictly below `root` (whole segments). Both are normalized.
fn is_inside(root: &str, path: &str) -> bool {
    let rp = dir_prefix(root);
    path.len() > rp.len() && key(path).starts_with(&key(&rp))
}

fn same_path(a: &str, b: &str) -> bool {
    key(a) == key(b)
}

/// Remainder of `path` below `root`; the caller guarantees [`is_inside`].
fn remainder<'a>(root: &str, path: &'a str) -> &'a str {
    &path[dir_prefix(root).len()..]
}

/// Absolute entries are normalized as they are; relative ones are resolved
/// against the main root. Drive-relative forms (`E:`, `E:dev`) are unusable.
fn resolve_entry(main: &str, entry: &str) -> Option<String> {
    let entry = entry.trim();
    if entry.is_empty() {
        return None;
    }
    if looks_absolute(entry) {
        return normalize_abs(entry);
    }
    if is_drive_path(entry) {
        return None;
    }
    normalize_abs(&format!("{}/{}", main, entry))
}

impl Roots {
    /// Build the root set from the main root and raw config entries.
    ///
    /// - `main_root` must be absolute; if it is not, it is kept verbatim (with
    ///   `\` → `/`) and every entry is dropped, i.e. the result is single-root.
    /// - A relative entry is resolved against the main root (`../lib`).
    /// - Dropped, never an error: empty entries, entries that cannot be
    ///   normalized, duplicates (first wins; drive paths compare
    ///   case-insensitively), the main root itself, anything inside the main
    ///   root (the main walk already covers it), any ancestor of the main root
    ///   (it would index the main root twice), and an entry inside another
    ///   kept entry (the outer one covers it).
    /// - Order of the surviving entries is preserved.
    pub fn new(main_root: &str, entries: &[String]) -> Roots {
        let Some(main) = normalize_abs(main_root) else {
            return Roots { main: main_root.replace('\\', "/"), extras: Vec::new() };
        };
        let mut extras: Vec<String> = Vec::new();
        for entry in entries {
            let Some(e) = resolve_entry(&main, entry) else { continue };
            if same_path(&e, &main) || is_inside(&main, &e) || is_inside(&e, &main) {
                continue;
            }
            if extras.iter().any(|k| same_path(k, &e) || is_inside(k, &e)) {
                continue;
            }
            extras.retain(|k| !is_inside(&e, k));
            extras.push(e);
        }
        Roots { main, extras }
    }

    /// [`Roots::load_with`] with [`HomePolicy::from_env`]. Every caller that
    /// serves files (indexing, MCP path checks) goes through here.
    pub fn load(main_root: &str) -> Roots {
        Self::load_with(main_root, &HomePolicy::from_env())
    }

    /// Read `<main_root>/.comp/config.json` and build the root set from
    /// `additional_paths` followed by `workspace_folders` (string arrays;
    /// anything else is ignored). Dropped with a warning in the log:
    /// - entries that are not existing directories;
    /// - entries `policy` blocks, judged on the resolved entry and, when it
    ///   can be canonicalized, on the canonical form too (a symlink into
    ///   `~/.ssh` is refused like `~/.ssh` itself);
    /// - everything [`Roots::new`] drops.
    ///
    /// A missing or malformed config yields a single-root set. The main root
    /// is never judged by `policy`.
    pub fn load_with(main_root: &str, policy: &HomePolicy) -> Roots {
        let Some(main) = normalize_abs(main_root) else {
            return Roots::new(main_root, &[]);
        };
        let path = std::path::Path::new(main_root).join(".comp/config.json");
        let json: serde_json::Value = std::fs::read_to_string(path)
            .ok()
            .and_then(|c| serde_json::from_str(&c).ok())
            .unwrap_or(serde_json::Value::Null);

        let mut entries: Vec<String> = Vec::new();
        for entry_key in ["additional_paths", "workspace_folders"] {
            if let Some(arr) = json[entry_key].as_array() {
                for v in arr.iter().filter_map(|v| v.as_str()) {
                    match resolve_entry(&main, v) {
                        Some(resolved) if std::path::Path::new(&resolved).is_dir() => {
                            let canonical = std::path::Path::new(&resolved)
                                .canonicalize()
                                .map(|c| c.to_string_lossy().to_string())
                                .ok();
                            if policy.blocks(&resolved) || canonical.as_deref().is_some_and(|c| policy.blocks(c)) {
                                log::warn!("Ignoring additional root in a hidden folder of the home directory: {}", v);
                            } else {
                                entries.push(resolved);
                            }
                        }
                        _ => log::warn!("Ignoring additional root that is not an existing directory: {}", v),
                    }
                }
            }
        }
        let roots = Roots::new(main_root, &entries);
        for entry in &entries {
            if !roots.extras.iter().any(|kept| same_path(kept, entry)) {
                log::warn!(
                    "Ignoring additional root {}: it is the workspace root, inside it, a parent of it, or covered by another entry",
                    entry
                );
            }
        }
        roots
    }

    // WHY test-only (also owner_of_stored / stored_to_abs): production code only
    // maps paths one way (locate); the tests use these to check the round trip.
    #[cfg(test)]
    pub fn main(&self) -> &str {
        &self.main
    }

    pub fn extras(&self) -> &[String] {
        &self.extras
    }

    /// Map an absolute file path to its owner and stored path.
    ///
    /// - The path is normalized first, so `..` cannot be used to leave a root.
    /// - Containment is by whole segments: `E:/dev/app2/x` is not inside
    ///   `E:/dev/app`.
    /// - Main root: stored path is the remainder (`src/a.rs`).
    /// - Additional root: stored path is that root's normalized string, `/`,
    ///   then the remainder with the caller's casing — so two spellings of the
    ///   root (`e:\DEV\lib`, `E:/dev/lib`) give the same stored path.
    /// - `None` for a path outside every root, a non-absolute path, and a
    ///   path equal to a root itself (a root is a directory, not a file).
    pub fn locate(&self, abs_path: &str) -> Option<Located> {
        let p = normalize_abs(abs_path)?;
        if is_inside(&self.main, &p) {
            return Some(Located { owner: Owner::Main, stored_path: remainder(&self.main, &p).to_string() });
        }
        for (i, extra) in self.extras.iter().enumerate() {
            if is_inside(extra, &p) {
                return Some(Located {
                    owner: Owner::Extra(i),
                    stored_path: format!("{}{}", dir_prefix(extra), remainder(extra, &p)),
                });
            }
        }
        None
    }

    /// Owner of a stored path, or `None` when it belongs to no current root
    /// (an absolute stored path left behind by a root that was removed from
    /// the config). A relative stored path is always [`Owner::Main`].
    #[cfg(test)]
    pub fn owner_of_stored(&self, stored_path: &str) -> Option<Owner> {
        if !looks_absolute(stored_path) {
            return Some(Owner::Main);
        }
        let p = normalize_abs(stored_path)?;
        self.extras.iter().position(|extra| is_inside(extra, &p)).map(Owner::Extra)
    }

    /// Whether `stored_path` is spelled the way the current roots would store
    /// it. A relative path always is. An absolute one is only when
    /// [`Roots::locate`] maps it to itself — so an entry of a removed root, an
    /// entry written under another spelling of its root (`E:/DEV/lib/a.rs`
    /// while the root is now `E:/dev/lib`), and an absolute alias of a
    /// main-root file (`E:/dev/app/src/a.rs`) are all not current.
    pub fn is_current_stored(&self, stored_path: &str) -> bool {
        if !looks_absolute(stored_path) {
            return true;
        }
        self.locate(stored_path).is_some_and(|l| l.stored_path == stored_path)
    }

    /// Absolute path of a stored path: a relative one is joined to the main
    /// root; an absolute one is returned normalized if a current additional
    /// root owns it. `None` when no current root owns it, or when a relative
    /// stored path climbs out of the main root with `..`.
    #[cfg(test)]
    pub fn stored_to_abs(&self, stored_path: &str) -> Option<String> {
        if looks_absolute(stored_path) {
            return match self.owner_of_stored(stored_path)? {
                Owner::Extra(_) => normalize_abs(stored_path),
                Owner::Main => None,
            };
        }
        let joined = normalize_abs(&format!("{}/{}", self.main, stored_path))?;
        is_inside(&self.main, &joined).then_some(joined)
    }
}

#[cfg(test)]
#[path = "roots_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "home_policy_tests.rs"]
mod home_policy_tests;
