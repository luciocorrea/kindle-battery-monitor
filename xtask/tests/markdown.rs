//! Checker CLI behavior, preserving the independently verified Python fixtures.
use std::{fs, process::Command};

fn check(case: &str) {
    let fixtures: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/markdown.json")).expect("baseline corpus");
    let mut count = 0;
    for fixture in fixtures
        .as_array()
        .expect("cases")
        .iter()
        .filter(|f| f["case"] == case)
    {
        count += 1;
        let tree = tempfile::Builder::new()
            .prefix("kindle docs ")
            .tempdir()
            .expect("tree");
        for (name, text) in fixture["files"].as_object().expect("files") {
            let path = tree.path().join(name);
            fs::create_dir_all(path.parent().expect("parent")).expect("directories");
            fs::write(path, text.as_str().expect("UTF-8 fixture")).expect("document");
        }
        let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
            .args(["check-links", "--root"])
            .arg(tree.path())
            .output()
            .expect("checker");
        assert_eq!(
            output.status.code(),
            Some(fixture["status"].as_i64().expect("status") as i32),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            fixture["stdout"].as_str().expect("stdout")
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stderr),
            fixture["stderr"].as_str().expect("stderr")
        );
    }
    assert!(count > 0, "undiscovered baseline {case}");
}
#[test]
fn test_relative_targets_resolve_from_the_referring_document() {
    check("test_relative_targets_resolve_from_the_referring_document");
}
#[test]
fn test_missing_targets_and_fragments_report_source_line_and_target() {
    check("test_missing_targets_and_fragments_report_source_line_and_target");
}
#[test]
fn test_literal_code_and_external_schemes_are_not_local_links() {
    check("test_literal_code_and_external_schemes_are_not_local_links");
}
#[test]
fn test_reference_links_images_and_encoded_paths() {
    check("test_reference_links_images_and_encoded_paths");
}

#[test]
fn test_heading_emphasis_is_removed_but_literal_underscores_are_preserved() {
    check("test_heading_emphasis_is_removed_but_literal_underscores_are_preserved");
}

#[test]
fn test_escaped_backticks_do_not_hide_real_links() {
    check("test_escaped_backticks_do_not_hide_real_links");
}

#[test]
fn test_backslash_inside_code_does_not_escape_the_closing_backtick() {
    check("test_backslash_inside_code_does_not_escape_the_closing_backtick");
}

#[test]
fn test_angle_destinations_do_not_require_balanced_parentheses() {
    check("test_angle_destinations_do_not_require_balanced_parentheses");
}

#[test]
fn test_blockquoted_fences_hide_code_but_not_following_document_links() {
    check("test_blockquoted_fences_hide_code_but_not_following_document_links");
}

#[test]
fn test_reference_labels_accept_escaped_closing_brackets() {
    check("test_reference_labels_accept_escaped_closing_brackets");
}

#[test]
fn test_broken_reference_images_and_undefined_references_fail() {
    check("test_broken_reference_images_and_undefined_references_fail");
}

#[test]
fn test_only_root_and_maintained_docs_are_checked() {
    check("test_only_root_and_maintained_docs_are_checked");
}

#[test]
fn test_heading_fragments_include_formatting_unicode_and_duplicate_suffixes() {
    check("test_heading_fragments_include_formatting_unicode_and_duplicate_suffixes");
}

#[test]
fn test_unicode_reference_labels_use_full_casefold() {
    check("test_unicode_reference_labels_use_full_casefold");
}
