#[macro_use]
extern crate version_sync;

#[test]
fn test_readme_deps() {
    assert_markdown_deps_updated!("../README.md");
}

// replace ignore with panic cfg gate when https://github.com/rust-lang/rust/pull/74754 is merged
#[test]
#[ignore]
fn test_html_root_url() {
    assert_html_root_url_updated!("src/lib.rs");
}
