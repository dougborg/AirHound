//! Provenance of this backend executable. Package versions are not release tags.
use serde_json::{json, Value};

fn nonempty(value: &'static str) -> Option<&'static str> {
    (!value.is_empty()).then_some(value)
}

pub fn current() -> Value {
    let revision = nonempty(env!("AIRHOUND_GIT_REVISION"));
    let tag = nonempty(env!("AIRHOUND_SOURCE_TAG"));
    let modified = match env!("AIRHOUND_SOURCE_MODIFIED") {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    };
    json!({
        "component":"airhound-hostd", "package_version":env!("CARGO_PKG_VERSION"),
        "git_revision":revision, "source_tag":tag, "tracked_source_modified":modified,
        "display_version":display(revision,tag,modified),
        "control_protocol":airhound_api::VERSION,
        "executable_path":std::env::current_exe().ok(),
    })
}
fn display(revision: Option<&str>, tag: Option<&str>, modified: Option<bool>) -> String {
    let source = match (tag, revision) {
        (Some(tag), _) => tag.to_owned(),
        (_, Some(revision)) => format!("git {}", revision.chars().take(12).collect::<String>()),
        _ => "source revision unknown".into(),
    };
    format!(
        "{source}{}",
        match modified {
            Some(true) => " (modified tracked source)",
            None => " (source state unknown)",
            Some(false) => "",
        }
    )
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    #[test]
    fn release_label_requires_tag_and_dirty_state_is_explicit() {
        assert_eq!(
            display(Some("0123456789abcdef"), None, Some(false)),
            "git 0123456789ab"
        );
        assert_eq!(
            display(
                Some("0123456789abcdef"),
                Some("v0.2.0-preview.1"),
                Some(true)
            ),
            "v0.2.0-preview.1 (modified tracked source)"
        );
        assert_eq!(
            display(None, None, None),
            "source revision unknown (source state unknown)"
        );
        let actual = current();
        assert_eq!(actual["control_protocol"], airhound_api::VERSION);
        assert_eq!(actual["package_version"], env!("CARGO_PKG_VERSION"));
    }
}
