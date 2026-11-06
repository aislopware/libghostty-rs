//! Supplies libghostty-vt to the link, from the first of:
//!
//! 1. `GHOSTTY_SOURCE_DIR`: build that Ghostty checkout with Zig. The only
//!    path that runs Zig, for whoever bumps the pin or hacks on Ghostty.
//! 2. `LIBGHOSTTY_VT_SYS_LIB_DIR`: link an unpacked library as it is, for
//!    offline machines and CI caches.
//! 3. The `pkg-config` feature: an installed library.
//! 4. Default: the prebuilt library for `TARGET` listed in `prebuilt.toml`,
//!    downloaded once, checked against its sha256 and cached outside `target/`.
//!
//! The prebuilt path needs no Zig at all, and never falls back to Zig on its
//! own: a stale or silently source-built library costs far more than a loud
//! failure that names the fix.

use std::collections::BTreeMap;
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Pinned ghostty commit. The prebuilt workflow reads these three constants
/// from this file, and `prebuilt.toml` names the commit its libraries were
/// built from, so one edit here moves the source, the bindings and the
/// binaries together.
const GHOSTTY_REPO: &str = "https://github.com/aislopware/ghostty.git";
const GHOSTTY_COMMIT: &str = "031685561440826283f81d5a6a4c4965e63e87bc";

/// The Zig the pin requires (`minimum_zig_version` in its build.zig.zon). The
/// prebuilt workflow installs exactly this one; a source build only warns on
/// another, since a newer patch release usually builds the same library.
const ZIG_VERSION: &str = "0.17.0";

/// File name of the static archive on Windows. Ghostty installs it under this
/// name for every Windows ABI so it does not collide with `ghostty-vt.lib`,
/// the import library for `ghostty-vt.dll`. Validation and link emission must
/// agree on it, or the build silently links the import library instead.
const WINDOWS_STATIC_LIB_FILE: &str = "ghostty-vt-static.lib";

/// Written into a cached prebuilt entry, holding its sha256, as the last step
/// before the entry is published by a rename. An entry without it, or with
/// another hash in it, is damaged and fetched again.
const PREBUILT_STAMP_FILE: &str = ".libghostty-vt-sha256";

/// Every environment variable this script reads, so Cargo reruns it when any
/// of them changes.
const WATCHED_ENV: &[&str] = &[
    "DOCS_RS",
    "TARGET",
    "HOST",
    "DEBUG",
    "OPT_LEVEL",
    "GHOSTTY_SOURCE_DIR",
    "GHOSTTY_ZIG_SYSTEM_DIR",
    "ZIG",
    "LIBGHOSTTY_VT_SYS_CPU",
    "LIBGHOSTTY_VT_SYS_OPTIMIZE",
    "LIBGHOSTTY_VT_SYS_LIB_DIR",
    "LIBGHOSTTY_VT_SYS_CACHE_DIR",
    "HOME",
    "XDG_CACHE_HOME",
    "LOCALAPPDATA",
    // Read by the Zig build of a source checkout for the Apple SDK and
    // deployment targets.
    "SDKROOT",
    "MACOSX_DEPLOYMENT_TARGET",
    "IPHONEOS_DEPLOYMENT_TARGET",
];

#[derive(Clone, Copy)]
enum LinkMode {
    Dynamic,
    Static,
}

impl LinkMode {
    fn current() -> Self {
        if cfg!(feature = "link-dynamic") {
            Self::Dynamic
        } else {
            Self::Static
        }
    }

    fn artifact_kind(self) -> &'static str {
        match self {
            Self::Dynamic => "shared library",
            Self::Static => "static library",
        }
    }

    fn matches_library(self, target: &str, file_name: &str) -> bool {
        match self {
            Self::Dynamic => {
                if target.contains("darwin") {
                    file_name.starts_with("libghostty-vt") && file_name.ends_with(".dylib")
                } else if target.contains("windows") {
                    file_name == "ghostty-vt.lib"
                        || file_name == "ghostty-vt.dll"
                        || file_name == "libghostty-vt.dll.lib"
                        || file_name == "libghostty-vt.dll.a"
                } else {
                    file_name == "libghostty-vt.so" || file_name.starts_with("libghostty-vt.so.")
                }
            }
            Self::Static => file_name == static_lib_file(target),
        }
    }

    #[cfg(feature = "pkg-config")]
    fn pkg_config_name(self) -> &'static str {
        match self {
            Self::Dynamic => "libghostty-vt",
            Self::Static => "libghostty-vt-static",
        }
    }
}

/// The file name of the static archive Ghostty installs for `target`.
fn static_lib_file(target: &str) -> &'static str {
    if target.contains("windows") {
        WINDOWS_STATIC_LIB_FILE
    } else {
        "libghostty-vt.a"
    }
}

fn main() {
    // docs.rs has no Zig toolchain and no network. The checked-in bindings in
    // src/bindings.rs are enough for generating documentation, so skip the
    // native library entirely when running under docs.rs.
    if env::var("DOCS_RS").is_ok() {
        return;
    }

    // Miri cannot load or call the native Ghostty library. The Miri suite stays
    // within Rust-owned seams, so there is nothing to build or link for it.
    if env::var("CARGO_CFG_MIRI").is_ok() {
        return;
    }

    let link_mode = LinkMode::current();
    // Cargo always sets TARGET for build scripts, so read it once here and
    // hand it to whichever path runs rather than re-reading it per call site.
    let target = env::var("TARGET").expect("TARGET must be set");

    // Wasm consumers link the archive into their own module, not a native
    // shared library. Check before every path, not just the source build.
    assert!(
        !target.starts_with("wasm") || matches!(link_mode, LinkMode::Static),
        "Wasm targets support static linking only; disable the link-dynamic feature"
    );

    for name in WATCHED_ENV {
        println!("cargo:rerun-if-env-changed={name}");
    }
    // Relative to the package root. The pin lives in this file and the
    // libraries for it in prebuilt.toml, so these two cover a new pin.
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=prebuilt.toml");

    // An explicit source override stays authoritative over every other path,
    // so local Ghostty checkouts remain easy to test against.
    if let Some(dir) = env_path("GHOSTTY_SOURCE_DIR") {
        build_from_source(link_mode, &target, &dir);
        return;
    }

    if let Some(dir) = env_path("LIBGHOSTTY_VT_SYS_LIB_DIR") {
        let include_dir = dir.join("include");
        emit_link(
            link_mode,
            &target,
            &dir,
            &include_dir,
            "LIBGHOSTTY_VT_SYS_LIB_DIR",
            true,
        );
        return;
    }

    // When the pkg-config feature is enabled, prefer an installed library over
    // the prebuilt one. libghostty is pre-1.0, so this crate intentionally does
    // not promise compatibility with every installed C API revision.
    #[cfg(feature = "pkg-config")]
    if try_pkg_config(link_mode, &target) {
        return;
    }

    link_prebuilt(link_mode, &target);
}

/// A non-empty environment variable. An empty one counts as unset, so
/// `FOO= cargo build` turns an override off rather than pointing it at "".
fn env_set(name: &str) -> Option<OsString> {
    env::var_os(name).filter(|value| !value.is_empty())
}

fn env_path(name: &str) -> Option<PathBuf> {
    env_set(name).map(PathBuf::from)
}

/// Check that a library and its headers are where the caller says, then emit
/// the link and include metadata for them.
///
/// `watch` reruns this script when one of the libraries changes or goes
/// away, as for a cache entry someone deleted. A source build's install
/// prefix is rewritten by every Zig run, so it is not watched there.
fn emit_link(
    link_mode: LinkMode,
    target: &str,
    prefix: &Path,
    include_dir: &Path,
    origin: &str,
    watch: bool,
) {
    let search_dirs = library_search_dirs(target, prefix);
    let libraries: Vec<PathBuf> = search_dirs
        .iter()
        .filter_map(|dir| fs::read_dir(dir).ok())
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| link_mode.matches_library(target, name))
        })
        .map(|entry| entry.path())
        .collect();
    assert!(
        !libraries.is_empty(),
        "expected the libghostty-vt {} for {target} in one of {search_dirs:?} (from {origin})",
        link_mode.artifact_kind(),
    );
    assert!(
        include_dir.join("ghostty").join("vt.h").exists(),
        "expected the libghostty-vt header at {} (from {origin})",
        include_dir.join("ghostty").join("vt.h").display()
    );

    if watch {
        for library in &libraries {
            println!("cargo:rerun-if-changed={}", library.display());
        }
    }
    // Tell the gen-bindings tool, a binary of this crate, which headers
    // belong to this exact build. Cargo keeps one build output per feature
    // set and profile, and ones from before a pin bump still hold the old
    // headers, so scanning the target directory can find stale ones.
    println!(
        "cargo:rustc-env=LIBGHOSTTY_VT_SYS_INCLUDE_DIR={}",
        include_dir.display()
    );
    for dir in &search_dirs {
        println!("cargo:rustc-link-search=native={}", dir.display());
    }
    match link_mode {
        LinkMode::Dynamic => println!("cargo:rustc-link-lib=dylib=ghostty-vt"),
        LinkMode::Static => emit_static_link_lib(target),
    }
    emit_include_metadata(&[include_dir.to_path_buf()]);
}

// ---------------------------------------------------------------------------
// Prebuilt libraries
// ---------------------------------------------------------------------------

/// One library in `prebuilt.toml`.
struct PrebuiltEntry {
    url: String,
    /// Lowercase hex sha256 of the archive at `url`.
    sha256: String,
}

/// What to say whenever the prebuilt path cannot supply a library.
fn prebuilt_fix() -> String {
    format!(
        "run the prebuilt workflow (.github/workflows/prebuilt.yml) for ghostty {GHOSTTY_COMMIT}, \
         which builds the libraries and fills crates/libghostty-vt-sys/prebuilt.toml; or check out \
         {GHOSTTY_REPO} at that commit and set GHOSTTY_SOURCE_DIR to build it with Zig {ZIG_VERSION}"
    )
}

/// Link the prebuilt library `prebuilt.toml` lists for `target`.
fn link_prebuilt(link_mode: LinkMode, target: &str) {
    // The prebuilt libraries are ReleaseFast for the baseline CPU. Someone
    // who asked for another optimize mode or CPU must get one or an error,
    // not quietly the prebuilt library.
    for name in ["LIBGHOSTTY_VT_SYS_CPU", "LIBGHOSTTY_VT_SYS_OPTIMIZE"] {
        assert!(
            env_set(name).is_none(),
            "{name} only applies to a source build, and the prebuilt libghostty-vt is ReleaseFast \
             for the baseline CPU: set GHOSTTY_SOURCE_DIR to a Ghostty checkout to build with it, \
             or unset {name}"
        );
    }
    assert!(
        matches!(link_mode, LinkMode::Static),
        "the prebuilt libghostty-vt is a static library; the link-dynamic feature needs \
         GHOSTTY_SOURCE_DIR (a source build) or LIBGHOSTTY_VT_SYS_LIB_DIR"
    );

    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set"));
    let manifest_path = manifest_dir.join("prebuilt.toml");
    let text = fs::read_to_string(&manifest_path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", manifest_path.display()));
    let (ghostty, entries) = parse_manifest(&text)
        .unwrap_or_else(|error| panic!("{}: {error}", manifest_path.display()));

    // A manifest left from the previous pin would hand out libraries the
    // checked-in bindings do not describe.
    assert!(
        ghostty == GHOSTTY_COMMIT,
        "prebuilt.toml lists libraries for ghostty {ghostty}, but build.rs pins {GHOSTTY_COMMIT}; {}",
        prebuilt_fix()
    );
    let Some(entry) = entries.get(target) else {
        panic!(
            "prebuilt.toml has no libghostty-vt for {target}; {}",
            prebuilt_fix()
        );
    };

    let root = cache_root().unwrap_or_else(|| {
        panic!(
            "found no cache directory for the prebuilt libghostty-vt (HOME, XDG_CACHE_HOME or \
             LOCALAPPDATA are unset); set LIBGHOSTTY_VT_SYS_CACHE_DIR"
        )
    });
    let dir = fetch_prebuilt(&root, entry, target);
    let include_dir = dir.join("include");
    emit_link(link_mode, target, &dir, &include_dir, "prebuilt.toml", true);
}

/// Parse `prebuilt.toml`: the top-level `ghostty` commit, and one table per
/// Rust target with its `url` and `sha256`. The workflow writes the file, so
/// only the subset of TOML it writes is accepted: comments, `[table]` or
/// `["table"]` headers and `key = "string"` lines. Unknown keys are ignored,
/// so the workflow may record more about each library.
fn parse_manifest(text: &str) -> Result<(String, BTreeMap<String, PrebuiltEntry>), String> {
    let mut tables = parse_tables(text)?;
    let mut top = tables.remove("").unwrap_or_default();
    let ghostty = top
        .remove("ghostty")
        .ok_or("missing the top-level ghostty = \"<commit>\"")?;

    let mut entries = BTreeMap::new();
    for (triple, mut table) in tables {
        let url = table
            .remove("url")
            .ok_or_else(|| format!("[{triple}] has no url"))?;
        let sha256 = table
            .remove("sha256")
            .ok_or_else(|| format!("[{triple}] has no sha256"))?
            .to_ascii_lowercase();
        if sha256.len() != 64 || !sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(format!(
                "[{triple}] sha256 must be 64 hex digits, not {sha256:?}"
            ));
        }
        entries.insert(triple, PrebuiltEntry { url, sha256 });
    }
    Ok((ghostty, entries))
}

/// The tables of a TOML subset, keyed by name; the top level is "".
fn parse_tables(text: &str) -> Result<BTreeMap<String, BTreeMap<String, String>>, String> {
    let mut tables: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    tables.insert(String::new(), BTreeMap::new());
    let mut current = String::new();
    for (index, raw) in text.lines().enumerate() {
        let number = index + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            let name = name.trim();
            let name = name
                .strip_prefix('"')
                .and_then(|rest| rest.strip_suffix('"'))
                .unwrap_or(name);
            if name.is_empty() || tables.contains_key(name) {
                return Err(format!("line {number}: empty or repeated table [{name}]"));
            }
            current = name.to_owned();
            tables.insert(current.clone(), BTreeMap::new());
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| format!("line {number}: expected key = \"value\""))?;
        let value = value
            .trim()
            .strip_prefix('"')
            .and_then(|rest| rest.strip_suffix('"'))
            .ok_or_else(|| format!("line {number}: the value must be a double-quoted string"))?;
        tables
            .get_mut(&current)
            .expect("the current table was inserted")
            .insert(key.trim().to_owned(), value.to_owned());
    }
    Ok(tables)
}

/// Where prebuilt libraries are cached: `LIBGHOSTTY_VT_SYS_CACHE_DIR`, else
/// the platform's per-user cache directory. Never under `target/`, so a
/// `cargo clean` or a fresh target dir costs no download, and every
/// workspace on the machine shares one copy per hash.
///
/// A build script is compiled for the host, so `cfg!` here is the host OS,
/// which is where the cache lives.
fn cache_root() -> Option<PathBuf> {
    if let Some(dir) = env_path("LIBGHOSTTY_VT_SYS_CACHE_DIR") {
        return Some(dir);
    }
    let base = if cfg!(target_os = "macos") {
        env_path("HOME")?.join("Library").join("Caches")
    } else if cfg!(windows) {
        env_path("LOCALAPPDATA")?
    } else {
        // The XDG spec ignores a relative XDG_CACHE_HOME.
        env_path("XDG_CACHE_HOME")
            .filter(|dir| dir.is_absolute())
            .or_else(|| env_path("HOME").map(|home| home.join(".cache")))?
    };
    Some(base.join("libghostty-vt"))
}

/// The cache entry `<root>/<sha256>/` for `entry`, downloaded, checked and
/// unpacked first if it is not there yet.
///
/// The archive is checked against the manifest's hash before anything is
/// unpacked, and the entry is published by renaming a finished staging
/// directory into place, so a concurrent build sees either nothing or a
/// complete entry, and an interrupted one leaves only a stray `.staging-*`.
fn fetch_prebuilt(root: &Path, entry: &PrebuiltEntry, target: &str) -> PathBuf {
    let dir = root.join(&entry.sha256);
    if is_cached(&dir, &entry.sha256, target) {
        return dir;
    }

    fs::create_dir_all(root)
        .unwrap_or_else(|error| panic!("failed to create {}: {error}", root.display()));
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let unique = format!("{}-{}-{nanos}", &entry.sha256[..16], std::process::id());
    let archive = root.join(format!(".download-{unique}.tar.gz"));
    let staging = root.join(format!(".staging-{unique}"));

    let unpacked = download_and_unpack(entry, target, &archive, &staging);
    let _ = fs::remove_file(&archive);
    if let Err(message) = unpacked {
        let _ = fs::remove_dir_all(&staging);
        panic!("{message}");
    }

    // An entry under this hash without a valid stamp is damaged (someone
    // edited or half-deleted it) and would refuse the rename.
    if dir.exists() && !is_cached(&dir, &entry.sha256, target) {
        let _ = fs::remove_dir_all(&dir);
    }
    if fs::rename(&staging, &dir).is_err() {
        // Another build published the same archive first; its copy is as good.
        let _ = fs::remove_dir_all(&staging);
        assert!(
            is_cached(&dir, &entry.sha256, target),
            "failed to publish the prebuilt libghostty-vt into {}",
            dir.display()
        );
    }
    dir
}

/// Whether `dir` is a complete cache entry for the archive with `sha256`.
///
/// The stamp is the last thing written before the entry is renamed into
/// place, after the archive's hash was checked, so it stands for that check;
/// the archive is not hashed again on every build script run.
fn is_cached(dir: &Path, sha256: &str, target: &str) -> bool {
    fs::read_to_string(dir.join(PREBUILT_STAMP_FILE)).is_ok_and(|stamp| stamp.trim() == sha256)
        && dir.join("lib").join(static_lib_file(target)).is_file()
        && dir.join("include").join("ghostty").join("vt.h").is_file()
}

/// Download `entry` to `archive` with curl, check its sha256, unpack it into
/// `staging` with tar and check what it holds.
///
/// curl and tar ship with macOS, Linux distributions and Windows 10 and
/// later, and a gzip tarball is the one archive all three tars unpack
/// without another tool (GNU tar shells out to `zstd`, which is often not
/// installed). The hash is computed here rather than by `shasum`,
/// `sha256sum` or `certutil`, which differ per OS in name and output.
fn download_and_unpack(
    entry: &PrebuiltEntry,
    target: &str,
    archive: &Path,
    staging: &Path,
) -> Result<(), String> {
    eprintln!(
        "Downloading libghostty-vt for {target} from {} ...",
        entry.url
    );
    let status = Command::new("curl")
        .args(["--fail", "--location", "--silent", "--show-error"])
        .args(["--retry", "3", "--output"])
        .arg(archive)
        .arg(&entry.url)
        .status()
        .map_err(|error| {
            format!(
                "failed to run curl to download {}: {error}; install curl, or unpack the archive \
                 yourself and set LIBGHOSTTY_VT_SYS_LIB_DIR to it",
                entry.url
            )
        })?;
    if !status.success() {
        return Err(format!(
            "curl failed ({status}) to download the libghostty-vt for {target} from {}; {}",
            entry.url,
            prebuilt_fix()
        ));
    }

    let bytes = fs::read(archive)
        .map_err(|error| format!("failed to read {}: {error}", archive.display()))?;
    let actual = sha256_hex(&bytes);
    if actual != entry.sha256 {
        return Err(format!(
            "the libghostty-vt for {target} from {} has sha256 {actual}, but prebuilt.toml \
             expects {}: the release asset or the manifest is wrong, so nothing was used; {}",
            entry.url,
            entry.sha256,
            prebuilt_fix()
        ));
    }

    fs::create_dir_all(staging)
        .map_err(|error| format!("failed to create {}: {error}", staging.display()))?;
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(staging)
        .status()
        .map_err(|error| format!("failed to run tar to unpack {}: {error}", archive.display()))?;
    if !status.success() {
        return Err(format!(
            "tar failed ({status}) to unpack {}",
            archive.display()
        ));
    }

    // The hash proves the archive is the one the manifest names; the meta
    // file proves the manifest named the right library for this build.
    let meta_path = staging.join("meta.toml");
    let meta = fs::read_to_string(&meta_path)
        .map_err(|error| format!("{} holds no meta.toml: {error}", entry.url))?;
    let mut meta = parse_tables(&meta)
        .map_err(|error| format!("meta.toml in {}: {error}", entry.url))?
        .remove("")
        .unwrap_or_default();
    for (key, expected) in [("ghostty", GHOSTTY_COMMIT), ("target", target)] {
        let found = meta.remove(key).unwrap_or_default();
        if found != expected {
            return Err(format!(
                "{} was built with {key} = {found:?}, but this build needs {expected:?}; {}",
                entry.url,
                prebuilt_fix()
            ));
        }
    }
    if !is_complete_unpack(staging, target) {
        return Err(format!(
            "{} does not hold lib/{} and include/ghostty/vt.h",
            entry.url,
            static_lib_file(target)
        ));
    }

    fs::write(staging.join(PREBUILT_STAMP_FILE), &entry.sha256).map_err(|error| {
        format!(
            "failed to write the stamp in {}: {error}",
            staging.display()
        )
    })
}

fn is_complete_unpack(dir: &Path, target: &str) -> bool {
    dir.join("lib").join(static_lib_file(target)).is_file()
        && dir.join("include").join("ghostty").join("vt.h").is_file()
}

/// SHA-256 (FIPS 180-4) of `data`, as lowercase hex.
///
/// Twenty lines here beat a build dependency for every consumer, and beat
/// parsing three platforms' hashing tools. It runs once per download, never
/// on a cache hit.
fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut state: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    // The last partial block, the 0x80 marker, zero padding and the message
    // length in bits make one or two final blocks.
    let full = data.len() - data.len() % 64;
    let mut tail = data[full..].to_vec();
    tail.push(0x80);
    while tail.len() % 64 != 56 {
        tail.push(0);
    }
    tail.extend_from_slice(&(data.len() as u64).wrapping_mul(8).to_be_bytes());

    let (blocks, _) = data[..full].as_chunks::<64>();
    let (tail_blocks, _) = tail.as_chunks::<64>();
    for block in blocks.iter().chain(tail_blocks) {
        let mut schedule = [0u32; 64];
        for (word, bytes) in schedule.iter_mut().zip(block.as_chunks::<4>().0) {
            *word = u32::from_be_bytes(*bytes);
        }
        for i in 16..64 {
            let s0 = schedule[i - 15].rotate_right(7)
                ^ schedule[i - 15].rotate_right(18)
                ^ (schedule[i - 15] >> 3);
            let s1 = schedule[i - 2].rotate_right(17)
                ^ schedule[i - 2].rotate_right(19)
                ^ (schedule[i - 2] >> 10);
            schedule[i] = schedule[i - 16]
                .wrapping_add(s0)
                .wrapping_add(schedule[i - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = state;
        for (k, w) in K.iter().zip(schedule.iter()) {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(*k)
                .wrapping_add(*w);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (word, value) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *word = word.wrapping_add(value);
        }
    }
    state.iter().map(|word| format!("{word:08x}")).collect()
}

// ---------------------------------------------------------------------------
// Source builds
// ---------------------------------------------------------------------------

/// Build libghostty-vt from a Ghostty checkout with Zig. The zig build itself
/// generates shared and static artifacts plus pkg-config files in
/// `share/pkgconfig/`.
fn build_from_source(link_mode: LinkMode, target: &str, ghostty_dir: &Path) {
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR must be set"));
    let host = env::var("HOST").expect("HOST must be set");

    assert!(
        ghostty_dir.join("build.zig").exists(),
        "GHOSTTY_SOURCE_DIR does not contain build.zig: {}",
        ghostty_dir.display()
    );
    watch_source_revision(ghostty_dir);

    // Loud on purpose: a source build is minutes of Zig, and the reason a
    // build is slow or links a different library should never be a mystery
    // (a GHOSTTY_SOURCE_DIR left in a shell or a .cargo/config.toml).
    println!(
        "cargo:warning=libghostty-vt: building from source in {} with Zig, because \
         GHOSTTY_SOURCE_DIR is set; unset it to link the prebuilt library",
        ghostty_dir.display()
    );
    if let Some(revision) = command_output(
        "git",
        &["-C", &ghostty_dir.to_string_lossy(), "rev-parse", "HEAD"],
    ) && revision != GHOSTTY_COMMIT
    {
        println!(
            "cargo:warning=libghostty-vt: {} is at {revision}, not the pinned {GHOSTTY_COMMIT}; \
             the checked-in bindings describe the pin",
            ghostty_dir.display()
        );
    }

    // Users can override the zig command with the `ZIG` env variable.
    // We default to assuming zig is in path.
    let zig = env_set("ZIG").unwrap_or_else(|| "zig".into());
    let zig_version = command_output(&zig, &["version"]).unwrap_or_else(|| {
        panic!(
            "GHOSTTY_SOURCE_DIR is set, so libghostty-vt is built with Zig, but `{}` did not run; \
             install Zig {ZIG_VERSION} or point ZIG at it, or unset GHOSTTY_SOURCE_DIR to link the \
             prebuilt library",
            zig.to_string_lossy()
        )
    });
    if zig_version != ZIG_VERSION {
        println!(
            "cargo:warning=libghostty-vt: building with Zig {zig_version}; the pin is built and \
             published with Zig {ZIG_VERSION}"
        );
    }

    // Build libghostty-vt via zig.
    let install_prefix = out_dir.join("ghostty-install");
    // Upstream installs the Wasm archive and standalone module, but not
    // headers. Use the same source revision rather than a host installation.
    let include_dir = if target.starts_with("wasm") {
        ghostty_dir.join("include")
    } else {
        install_prefix.join("include")
    };

    // Zig's cache is content-addressed and safe to share between concurrent
    // builds, so every target dir, profile and checkout on the machine shares
    // one: a second target dir rebuilds in seconds instead of minutes, with
    // no key of our own to get wrong. A sandboxed package build (which sets
    // GHOSTTY_ZIG_SYSTEM_DIR) has no writable home, so it keeps OUT_DIR.
    let zig_cache_dir = if env_set("GHOSTTY_ZIG_SYSTEM_DIR").is_some() {
        out_dir.join("zig-cache")
    } else {
        cache_root().map_or_else(|| out_dir.join("zig-cache"), |root| root.join("zig-cache"))
    };
    let zig_global_cache_dir = out_dir.join("zig-global-cache");

    let optimize = zig_optimize_mode();
    let cpu = env_set("LIBGHOSTTY_VT_SYS_CPU").map_or_else(
        || "baseline".to_owned(),
        |cpu| cpu.to_string_lossy().into_owned(),
    );

    // iOS builds go through ghostty's emit-xcframework path instead of a flat
    // `-Dtarget=<ios> --sysroot=<sdk>` invocation. The flat build breaks down
    // for iOS: a generic target baseline can't compile simdutf's always_inline
    // NEON intrinsics, and `--sysroot` applies globally so it leaks into the
    // native codegen tools ghostty runs mid-build. The xcframework path runs
    // host-native and configures each Apple platform itself, so we build that
    // and pull out the library we need afterwards. The prebuilt workflow does
    // the same.
    let ios_platform = ios_xcframework_platform(target);
    if ios_platform.is_some() {
        // Ghostty only emits the xcframework when zig itself runs on macOS
        // (it shells out to xcodebuild). Without this check a Linux cross
        // build would silently produce a host-native library and then fail on
        // a confusing missing-xcframework assertion after the full build.
        assert!(
            host.contains("apple-darwin"),
            "building for {target} requires a macOS host with Xcode and the iOS SDK \
             (ghostty's emit-xcframework path runs host-native); host is {host}"
        );
        // The xcframework contains only static archives, and the flat layout
        // below is populated from one of them. Fail up front instead of
        // letting the shared-library search fail with a misleading message.
        assert!(
            matches!(link_mode, LinkMode::Static),
            "building for {target} supports static linking only; \
             disable the link-dynamic feature"
        );
    }

    let mut build = Command::new(&zig);
    build
        .arg("build")
        .arg("-Demit-lib-vt=true")
        .arg(format!("-Doptimize={optimize}"))
        // Cargo artifacts may run on older CPUs than the build host. Without
        // an explicit CPU model, Zig may emit host-specific instructions that
        // make distributed binaries fail with an illegal instruction. Users
        // building for a known machine can explicitly request `native` or a
        // named Zig CPU model through LIBGHOSTTY_VT_SYS_CPU.
        //
        // For iOS builds this only affects the host-native flat artifacts:
        // ghostty resolves its own per-platform targets for the xcframework
        // slices, so -Dcpu does not leak into them.
        .arg(format!("-Dcpu={cpu}"))
        .arg(if ios_platform.is_some() {
            "-Demit-xcframework=true"
        } else {
            "-Demit-xcframework=false"
        })
        .arg("-Dapp-runtime=none")
        .arg("--prefix")
        .arg(&install_prefix)
        .arg("--cache-dir")
        .arg(&zig_cache_dir)
        .current_dir(ghostty_dir);

    // Package managers can provide Ghostty's Zig package cache ahead of time
    // and ask Zig to resolve packages from that immutable store path instead
    // of fetching during this Cargo build script. Those sandboxes usually
    // have no writable home either, so the global cache goes under OUT_DIR.
    // Zig 0.17 removed `--global-cache-dir`; the global cache can only be
    // redirected through the environment now (as ghostty's own
    // nix/build-support/update-mirror.nu does).
    if let Some(dir) = env_path("GHOSTTY_ZIG_SYSTEM_DIR") {
        assert!(
            dir.exists(),
            "GHOSTTY_ZIG_SYSTEM_DIR does not exist: {}",
            dir.display()
        );
        build
            .arg("--system")
            .arg(&dir)
            .env("ZIG_GLOBAL_CACHE_DIR", &zig_global_cache_dir);
    }

    // Pass -Dtarget only for non-iOS cross targets; native builds let zig
    // auto-detect the host. iOS builds run host-native inside the xcframework
    // emit, so they must not pass -Dtarget or a global --sysroot.
    if target != host && ios_platform.is_none() {
        let zig_target = zig_target(target);
        build.arg(format!("-Dtarget={zig_target}"));
    }

    run(build, "zig build");

    // The emit also installs host-native flat artifacts; replace them with the
    // iOS library so the link emission below picks up the right arch.
    if let Some(platform) = ios_platform {
        extract_xcframework_lib(&install_prefix, platform);
    } else {
        warn_unused_xcframework(&install_prefix.join("lib"));
    }

    emit_link(
        link_mode,
        target,
        &install_prefix,
        &include_dir,
        "GHOSTTY_SOURCE_DIR",
        false,
    );
}

/// A command's trimmed standard output, when it ran and succeeded.
fn command_output(program: impl AsRef<OsStr>, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Emit the link directive for the static archive.
///
/// Zig names static archives `<name>.lib` on every Windows ABI, so neither
/// name rustc derives from a plain `static=ghostty-vt` finds the archive
/// there: MSVC resolves it to `ghostty-vt.lib`, the DLL import library, which
/// links but leaves a load-time dependency on `ghostty-vt.dll`; the GNU
/// targets look for `libghostty-vt.a`, which no Windows build produces, and
/// fail outright. Link the exact file name Ghostty installs instead, which
/// `+verbatim` passes through to rustc's archive lookup untouched.
fn emit_static_link_lib(target: &str) {
    if target.contains("windows") {
        println!("cargo:rustc-link-lib=static:+verbatim={WINDOWS_STATIC_LIB_FILE}");
    } else {
        println!("cargo:rustc-link-lib=static=ghostty-vt");
    }
}

fn warn_unused_xcframework(lib_dir: &Path) {
    let xcframework = lib_dir.join("ghostty-vt.xcframework");
    if xcframework.exists() {
        println!(
            "cargo:warning=unused libghostty-vt XCFramework emitted at {}; Cargo links the dylib or archive directly",
            xcframework.display()
        );
    }
}

#[cfg(feature = "pkg-config")]
fn try_pkg_config(link_mode: LinkMode, target: &str) -> bool {
    let mut config = pkg_config::Config::new();
    let lib = match link_mode {
        LinkMode::Dynamic => config.probe(link_mode.pkg_config_name()),
        LinkMode::Static => config
            .statik(true)
            .cargo_metadata(false)
            .probe(link_mode.pkg_config_name()),
    };
    let lib = match lib {
        Ok(lib) => lib,
        Err(_) => return false,
    };

    if let LinkMode::Static = link_mode {
        emit_static_pkg_config_metadata(&lib, target);
    }
    emit_include_metadata(&lib.include_paths);
    true
}

#[cfg(feature = "pkg-config")]
fn emit_static_pkg_config_metadata(lib: &pkg_config::Library, target: &str) {
    for path in &lib.link_paths {
        println!("cargo:rustc-link-search=native={}", path.display());
    }
    for path in &lib.link_files {
        if let Some(parent) = path.parent() {
            println!("cargo:rustc-link-search=native={}", parent.display());
        }
    }
    for path in &lib.framework_paths {
        println!("cargo:rustc-link-search=framework={}", path.display());
    }
    for framework in &lib.frameworks {
        println!("cargo:rustc-link-lib=framework={framework}");
    }

    emit_static_link_lib(target);
    for library in &lib.libs {
        if library != "ghostty-vt" {
            println!("cargo:rustc-link-lib={library}");
        }
    }
    for args in &lib.ld_args {
        if !args.is_empty() {
            println!("cargo:rustc-link-arg=-Wl,{}", args.join(","));
        }
    }
}

fn emit_include_metadata(include_paths: &[PathBuf]) {
    if include_paths.is_empty() {
        return;
    }

    let joined = env::join_paths(include_paths)
        .unwrap_or_else(|error| panic!("failed to join include paths for cargo metadata: {error}"));
    println!("cargo:include={}", joined.to_string_lossy());
}

/// Decide which Zig `OptimizeMode` a source build passes to `zig build`.
///
/// The `LIBGHOSTTY_VT_SYS_OPTIMIZE` environment variable overrides this unconditionally; accepted
/// values are the four Zig `OptimizeMode` names (`Debug`, `ReleaseSafe`, `ReleaseFast`,
/// `ReleaseSmall`).
///
/// Defaults to `ReleaseFast` for optimized builds. If `DEBUG` is `true` (as cargo sets for the
/// `dev` profile), `Debug` mode is used. Otherwise, if `OPT_LEVEL` is `s` or `z`, `ReleaseSmall`
/// is used. The prebuilt libraries are always `ReleaseFast`.
fn zig_optimize_mode() -> &'static str {
    if let Ok(override_mode) = env::var("LIBGHOSTTY_VT_SYS_OPTIMIZE") {
        return match override_mode.as_str() {
            "Debug" => "Debug",
            "ReleaseSafe" => "ReleaseSafe",
            "ReleaseFast" => "ReleaseFast",
            "ReleaseSmall" => "ReleaseSmall",
            other => panic!(
                "LIBGHOSTTY_VT_SYS_OPTIMIZE must be one of Debug, ReleaseSafe, ReleaseFast, ReleaseSmall (got '{other}')"
            ),
        };
    }

    if env::var("DEBUG").as_deref() == Ok("true") {
        return "Debug";
    }

    match env::var("OPT_LEVEL").as_deref() {
        Ok("s") | Ok("z") => "ReleaseSmall",
        _ => "ReleaseFast",
    }
}

/// Ask Cargo to rerun this script when the ghostty source it builds from
/// changes.
///
/// A git checkout is watched through its HEAD rather than its files. Moving a
/// submodule or checking out another commit rewrites HEAD, and HEAD pins every
/// tracked file, so it is the exact signal. Listing the source directory would
/// make Cargo stat thousands of files on every build. Uncommitted edits inside
/// the checkout are not seen; `touch build.zig` after making one.
///
/// A source without git metadata has no revision to watch, so the whole tree is
/// watched. That is slower, but a stale library from such a tree is worse.
fn watch_source_revision(src: &Path) {
    let Some(git_dir) = git_dir(src) else {
        println!("cargo:rerun-if-changed={}", src.display());
        return;
    };
    let head = git_dir.join("HEAD");
    println!("cargo:rerun-if-changed={}", head.display());

    let common_dir = fs::read_to_string(git_dir.join("commondir"))
        .map(|dir| git_dir.join(dir.trim()))
        .unwrap_or_else(|_| git_dir.clone());

    // A reftable repository keeps a stub HEAD and every ref in its tables.
    let reftable = common_dir.join("reftable");
    if reftable.is_dir() {
        println!("cargo:rerun-if-changed={}", reftable.display());
        return;
    }

    // On a branch, HEAD names a ref whose commit sits in a loose ref file or
    // in packed-refs. Cargo reruns on every build for a watched path that does
    // not exist, so a missing loose ref is watched through its nearest existing
    // parent directory, whose mtime moves when git writes the ref.
    let Ok(contents) = fs::read_to_string(&head) else {
        return;
    };
    let Some(reference) = contents.trim().strip_prefix("ref: ") else {
        return;
    };
    let packed_refs = common_dir.join("packed-refs");
    if packed_refs.exists() {
        println!("cargo:rerun-if-changed={}", packed_refs.display());
    }
    let loose_ref = common_dir.join(reference);
    if let Some(watched) = loose_ref
        .ancestors()
        .take_while(|path| *path != common_dir)
        .find(|path| path.exists())
    {
        println!("cargo:rerun-if-changed={}", watched.display());
    }
}

/// The git directory of a checkout: `.git` itself, or the directory a
/// `gitdir:` file points at, as submodules and linked worktrees use.
fn git_dir(src: &Path) -> Option<PathBuf> {
    let dot_git = src.join(".git");
    if dot_git.is_dir() {
        return Some(dot_git);
    }
    let contents = fs::read_to_string(&dot_git).ok()?;
    let target = contents.trim().strip_prefix("gitdir:")?.trim();
    Some(src.join(target))
}

fn run(mut command: Command, context: &str) {
    let status = command
        .status()
        .unwrap_or_else(|error| panic!("failed to execute {context}: {error}"));
    assert!(status.success(), "{context} failed with status {status}");
}

/// Returns directories to search for the library artifact.
/// On Windows, Zig may place the DLL in `bin/` and the import lib in `lib/`,
/// so both are included.
fn library_search_dirs(target: &str, install_prefix: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![install_prefix.join("lib")];
    if target.contains("windows") {
        dirs.push(install_prefix.join("bin"));
    }
    dirs
}

/// The platform directory inside `ghostty-vt.xcframework` for an iOS Rust
/// target, or `None` for targets that build directly via `-Dtarget`. Ghostty
/// emits an arm64-only simulator library (what the simulator runs on Apple
/// silicon), so x86_64-apple-ios is not supported here.
fn ios_xcframework_platform(target: &str) -> Option<&'static str> {
    match target {
        "aarch64-apple-ios" => Some("ios-arm64"),
        "aarch64-apple-ios-sim" => Some("ios-arm64-simulator"),
        _ => None,
    }
}

/// Copy an xcframework platform's static lib + headers into the flat
/// `<prefix>/lib/libghostty-vt.a` and `<prefix>/include` layout the link
/// emission expects, replacing the host-native artifacts the emit installed.
fn extract_xcframework_lib(install_prefix: &Path, platform: &str) {
    let platform_dir = install_prefix
        .join("lib")
        .join("ghostty-vt.xcframework")
        .join(platform);
    // Ghostty emits iOS xcframework slices only when it detects the iOS SDK,
    // silently skipping them otherwise, so a missing directory here almost
    // always means the SDK is absent rather than a build failure.
    assert!(
        platform_dir.is_dir(),
        "expected xcframework platform dir {platform} at {}; \
         ghostty emits iOS slices only when the iOS SDK is detected, \
         so install it via Xcode (Settings > Components)",
        platform_dir.display()
    );
    // ghostty names the Apple static libraries `libghostty-vt-fat.a`.
    let src_lib = platform_dir.join("libghostty-vt-fat.a");
    assert!(
        src_lib.exists(),
        "expected static lib at {}",
        src_lib.display()
    );

    let lib_dir = install_prefix.join("lib");
    let dest_lib = lib_dir.join("libghostty-vt.a");
    fs::copy(&src_lib, &dest_lib).unwrap_or_else(|error| {
        panic!(
            "failed to copy {} -> {}: {error}",
            src_lib.display(),
            dest_lib.display()
        )
    });

    // Headers are arch-independent, but prefer the platform's own copy.
    let headers = platform_dir.join("Headers");
    if headers.is_dir() {
        copy_dir_all(&headers, &install_prefix.join("include"));
    }
}

/// Recursively copy a directory tree (merging into `dst` if it exists).
fn copy_dir_all(src: &Path, dst: &Path) {
    fs::create_dir_all(dst)
        .unwrap_or_else(|error| panic!("failed to create {}: {error}", dst.display()));
    let entries = fs::read_dir(src)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", src.display()));
    for entry in entries {
        let entry = entry
            .unwrap_or_else(|error| panic!("failed to read entry in {}: {error}", src.display()));
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let file_type = entry
            .file_type()
            .unwrap_or_else(|error| panic!("failed to stat {}: {error}", from.display()));
        if file_type.is_dir() {
            copy_dir_all(&from, &to);
        } else {
            fs::copy(&from, &to)
                .unwrap_or_else(|error| panic!("failed to copy {}: {error}", from.display()));
        }
    }
}

fn zig_target(target: &str) -> String {
    let value = match target {
        "x86_64-unknown-linux-gnu" => "x86_64-linux-gnu",
        "x86_64-unknown-linux-musl" => "x86_64-linux-musl",
        "aarch64-unknown-linux-gnu" => "aarch64-linux-gnu",
        "aarch64-unknown-linux-musl" => "aarch64-linux-musl",
        "aarch64-apple-darwin" => "aarch64-macos-none",
        "x86_64-apple-darwin" => "x86_64-macos-none",
        "x86_64-pc-windows-gnu" => "x86_64-windows-gnu",
        "aarch64-pc-windows-gnullvm" => "aarch64-windows-gnu",
        "x86_64-pc-windows-msvc" => "x86_64-windows-msvc",
        "aarch64-pc-windows-msvc" => "aarch64-windows-msvc",
        "aarch64-linux-android" => "aarch64-linux-android",
        "x86_64-linux-android" => "x86_64-linux-android",
        "wasm32-unknown-unknown" => "wasm32-freestanding",
        "wasm64-unknown-unknown" => "wasm64-freestanding",
        other => panic!("unsupported Rust target for a source build: {other}"),
    };
    value.to_owned()
}
