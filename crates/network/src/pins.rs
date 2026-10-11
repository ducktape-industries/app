//! The revs this tool installs, read out of the app's Cargo.lock at build time (`build.rs`
//! includes this file): the core commit the app speaks and the modules commit whose programs it
//! seats.

/// The one commit of `ducktape-industries/<repo>` the lock names: the `#<commit>` of its
/// `source` lines. Two sources of one repo are two answers to "which node does the app speak",
/// so a lock that names more than one is refused, as is one that names none.
pub fn pinned(lock: &str, repo: &str) -> Result<String, String> {
    let wanted = format!("https://github.com/ducktape-industries/{repo}");
    let mut sources: Vec<&str> = lock
        .lines()
        .filter_map(|line| line.strip_prefix("source = \"git+")?.strip_suffix('"'))
        .filter(|url| {
            let repo_url = url.split(['?', '#']).next().unwrap_or(url);
            repo_url.trim_end_matches(".git") == wanted
        })
        .collect();
    sources.sort_unstable();
    sources.dedup();
    let [source] = sources[..] else {
        return Err(format!(
            "Cargo.lock names {} sources of {wanted}, not one: {sources:?}",
            sources.len()
        ));
    };
    let commit = source.rsplit_once('#').map(|(_, commit)| commit);
    match commit {
        Some(commit) if commit.len() == 40 && commit.bytes().all(|b| b.is_ascii_hexdigit()) => {
            Ok(commit.to_string())
        }
        _ => Err(format!("Cargo.lock's {source} names no full commit")),
    }
}

#[cfg(test)]
mod tests {
    use super::pinned;

    const CORE: &str = "00884abdcf72de444c856919aa007b603fc7d7b1";
    const MODULES: &str = "47cf81bc3ef542df057f7e482d18f631e51719fb";

    fn package(name: &str, source: &str) -> String {
        format!("[[package]]\nname = \"{name}\"\nversion = \"0.1.0\"\nsource = \"{source}\"\n\n")
    }

    #[test]
    fn each_repo_is_its_one_source_commit() {
        let core = format!("git+https://github.com/ducktape-industries/ducktape?rev={CORE}#{CORE}");
        let modules =
            format!("git+https://github.com/ducktape-industries/modules?rev={MODULES}#{MODULES}");
        let lock = [
            package("keystore", &core),
            package("keyscheme", &core),
            package("abi", &modules),
            package(
                "gpui-pre",
                "git+https://github.com/ducktape-industries/gpui-pre?rev=da49e2b0#da49e2b089cb5bf8f36fef940cf5c0b94df92138",
            ),
            package(
                "keyscheme-old",
                "git+https://github.com/ducktape-industries/ducktape-sdk?rev=7368657#736865710dcfa7c56f9834747287881c1c25d45d",
            ),
            package(
                "anyhow",
                "registry+https://github.com/rust-lang/crates.io-index",
            ),
        ]
        .concat();
        assert_eq!(pinned(&lock, "ducktape"), Ok(CORE.to_string()));
        assert_eq!(pinned(&lock, "modules"), Ok(MODULES.to_string()));
    }

    #[test]
    fn two_sources_of_one_repo_or_none_is_refused() {
        let lock = [
            package(
                "ducktape-home",
                "git+https://github.com/ducktape-industries/ducktape?branch=feat%2Fcapable-sandbox#665907ddc65ccada5f3e3ba0a95ae955414bfb3c",
            ),
            package(
                "keyscheme",
                "git+https://github.com/ducktape-industries/ducktape?rev=9dfffb06#9dfffb0662bd8a16293df8c75de8817a79b0810b",
            ),
        ]
        .concat();
        assert!(pinned(&lock, "ducktape").is_err());
        assert!(pinned(&lock, "modules").is_err());
    }
}
