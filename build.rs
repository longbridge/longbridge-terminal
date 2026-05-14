use std::io::Write;
use std::path::Path;

fn main() {
    // Windows default stack is 1MB, too small for debug async futures.
    #[cfg(windows)]
    println!("cargo:rustc-link-arg=/STACK:8388608");

    generate_special_counter_ids();
}

fn generate_special_counter_ids() {
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let dest = Path::new(&out_dir).join("special_counter_ids.rs");

    let paths = ["data/ETF.csv", "data/IX.csv", "data/WT.csv"];
    for path in &paths {
        println!("cargo:rerun-if-changed={path}");
    }
    let contents: Vec<String> = paths
        .iter()
        .map(|p| std::fs::read_to_string(p).unwrap())
        .collect();

    let mut set = phf_codegen::Set::new();
    for content in &contents {
        for line in content.lines() {
            let s = line.trim();
            if !s.is_empty() {
                set.entry(s);
            }
        }
    }

    let mut file = std::fs::File::create(&dest).unwrap();
    writeln!(
        file,
        "static SPECIAL_COUNTER_IDS: phf::Set<&'static str> = {};",
        set.build()
    )
    .unwrap();
}
