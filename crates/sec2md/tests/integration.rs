use std::path::PathBuf;

use sec2md::convert;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// For each `*.html` in `tests/fixtures/`:
///   - Convert it to Markdown.
///   - If a matching `*.md` exists, assert the output matches (snapshot check).
///   - If no `*.md` exists, write it (golden-file creation on first run).
#[test]
fn golden_files() {
    let dir = fixtures_dir();
    let mut html_files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("fixtures dir must exist")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("html"))
        .collect();
    html_files.sort();

    assert!(
        !html_files.is_empty(),
        "no *.html files found in tests/fixtures/"
    );

    // When UPDATE_GOLDEN=1, defer everything to the update_golden test.
    if std::env::var("UPDATE_GOLDEN").as_deref() == Ok("1") {
        return;
    }

    let mut failures: Vec<String> = Vec::new();

    for html_path in &html_files {
        let stem = html_path.file_stem().unwrap().to_string_lossy();
        let md_path = dir.join(format!("{stem}.md"));

        let html = std::fs::read_to_string(html_path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", html_path.display()));

        let output = convert(&html);

        if md_path.exists() {
            let expected = std::fs::read_to_string(&md_path)
                .unwrap_or_else(|e| panic!("failed to read {}: {e}", md_path.display()));
            if output != expected {
                failures.push(format!(
                    "{stem}: output differs from golden file (run with UPDATE_GOLDEN=1 to update)",
                ));
            }
        } else {
            std::fs::write(&md_path, &output)
                .unwrap_or_else(|e| panic!("failed to write {}: {e}", md_path.display()));
            eprintln!("created golden file: {}", md_path.display());
        }
    }

    if !failures.is_empty() {
        panic!("golden file mismatches:\n{}", failures.join("\n"));
    }
}

/// When UPDATE_GOLDEN=1 is set, regenerate all golden `.md` files unconditionally.
#[test]
fn update_golden() {
    if std::env::var("UPDATE_GOLDEN").as_deref() != Ok("1") {
        return;
    }

    let dir = fixtures_dir();
    let html_files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("fixtures dir must exist")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("html"))
        .collect();

    for html_path in &html_files {
        let stem = html_path.file_stem().unwrap().to_string_lossy();
        let md_path = dir.join(format!("{stem}.md"));
        let html = std::fs::read_to_string(html_path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", html_path.display()));
        let output = convert(&html);
        std::fs::write(&md_path, &output)
            .unwrap_or_else(|e| panic!("failed to write {}: {e}", md_path.display()));
        eprintln!("updated: {}", md_path.display());
    }
}
