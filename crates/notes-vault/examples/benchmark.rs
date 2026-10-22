//! Deterministic synthetic fixtures; no user vaults are touched.
use std::{fs, time::Instant};
fn main() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    println!("notes,cold_index_ms,warm_reconcile_ms,search_ms,graph_build_ms,local_graph_ms");
    for count in [1_000, 10_000, 50_000] {
        let root = temp.path().join(format!("vault-{count}"));
        fs::create_dir(&root)?;
        for i in 0..count {
            fs::write(
                root.join(format!("Note-{i}.md")),
                format!(
                    "---\ntags: [benchmark]\n---\n# Research {i}\n\nLocal-first writing and searchable knowledge. [[Note-{}]] [[Note-{}]] [[Note-{}]]\n",
                    (i + 1) % count,
                    (i + 7) % count,
                    (i + 42) % count
                ),
            )?;
        }
        let attachments = root.join("attachments");
        fs::create_dir(&attachments)?;
        for i in 0..1000 {
            fs::write(attachments.join(format!("image-{i}.bin")), [0u8; 128])?;
        }
        let db = temp.path().join(format!("index-{count}.sqlite"));
        let start = Instant::now();
        let indexed = notes_vault::rebuild_index(&db, &root)?;
        let cold = start.elapsed();
        assert_eq!(indexed.0, count);
        let start = Instant::now();
        notes_vault::reconcile_index(&db, &root)?;
        let warm = start.elapsed();
        let start = Instant::now();
        let hits = notes_vault::search(&db, "searchable")?;
        let search = start.elapsed();
        assert!(!hits.is_empty());
        let start = Instant::now();
        let graph = notes_vault::knowledge::Graph::load(&db, &root)?;
        let graph_time = start.elapsed();
        let start = Instant::now();
        let visible = graph.visible("", "", Some(&root.join("Note-0.md")), 2, true);
        let local = start.elapsed();
        assert!(visible.len() > 1);
        println!(
            "{count},{},{},{},{},{}",
            cold.as_millis(),
            warm.as_millis(),
            search.as_millis(),
            graph_time.as_millis(),
            local.as_millis()
        );
    }
    let text =
        "# Large document\n\n**Bold** and *emphasis* with [[links]] and `code`.\n\n".repeat(16000);
    let start = Instant::now();
    let parsed = notes_markdown::parse(&text);
    println!(
        "large_document_bytes={},parse_ms={},headings={}",
        text.len(),
        start.elapsed().as_millis(),
        parsed.headings.len()
    );
    if let Ok(status) = fs::read_to_string("/proc/self/status")
        && let Some(line) = status.lines().find(|line| line.starts_with("VmHWM:"))
    {
        println!("process_peak_memory: {line}");
    }
    Ok(())
}
