use super::*;

/// A CJK component name fits its column whole: the name column is sized by
/// cells, so the wide name renders `日本語` instead of being cut to `日…` by a
/// char-derived width.
#[test]
fn transition_lines_keep_a_wide_name_whole() {
    let lines = transition_lines(
        &[(
            "日本語".to_string(),
            "operational".to_string(),
            "degraded".to_string(),
        )],
        0,
        60,
    );
    let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
    assert_eq!(text, "日本語  → degraded");
}
