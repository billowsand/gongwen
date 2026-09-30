//! 依赖升级的手工性能对照；只在测试构建中存在，不计入常规 CI。

use std::hint::black_box;
use std::time::Instant;

fn sample<T>(count: usize, mut operation: impl FnMut() -> T) -> serde_json::Value {
    let mut micros = Vec::with_capacity(count);
    for _ in 0..count {
        let start = Instant::now();
        black_box(operation());
        micros.push(start.elapsed().as_secs_f64() * 1_000_000.0);
    }
    micros.sort_by(f64::total_cmp);
    serde_json::json!({
        "samples": count,
        "median_us": micros[count / 2],
        "min_us": micros[0],
        "max_us": micros[count - 1],
    })
}

#[test]
#[ignore = "依赖升级时用 release 构建单独执行，输出性能对照"]
fn dependency_performance_probe() {
    let _guard = crate::lexicon::segmenter::test_lock();
    let start = Instant::now();
    let jieba = jieba_rs::Jieba::new();
    let first_dictionary_us = start.elapsed().as_secs_f64() * 1_000_000.0;
    let sentence = "新舆处开展2026年专项整治行动，预算120万元，文号为公办〔2026〕17号。Alpha123与中文混排，保留公式$x^2$和用户词。";
    let body = sentence.repeat(80);
    let old = format!("# 专项工作报告\n\n{}", [sentence; 80].join("\n\n"));
    let new = old
        .replace("120万元", "130万元")
        .replace("专项整治", "专项检查");
    black_box(jieba.cut(&body, true));
    black_box(crate::redline::build(&old, &new));
    let dir = tempfile::tempdir().expect("性能样本目录");
    let path = dir.path().join("词表.xlsx");
    let config = crate::models::ProofreadConfig::default();
    crate::proofread_xlsx::to_xlsx(&config, &path).expect("准备词表");
    let report = serde_json::json!({
        "body_chars": body.chars().count(),
        "dictionary_first_us": first_dictionary_us,
        "dictionary_instances": sample(5, jieba_rs::Jieba::new),
        "segmentation": sample(30, || jieba.cut(&body, true)),
        "tagging": sample(30, || jieba.tag(&body, true)),
        "redline": sample(30, || crate::redline::build(&old, &new)),
        "xlsx_import": sample(30, || crate::proofread_xlsx::parse(&path).expect("回读词表")),
        "xlsx_export": sample(30, || crate::proofread_xlsx::to_xlsx(&config, &path).expect("导出词表")),
    });
    println!("DEPENDENCY_BENCH={report}");
    if let Ok(path) = std::env::var("GONGWEN_BENCH_OUTPUT") {
        std::fs::write(
            path,
            serde_json::to_vec_pretty(&report).expect("记录序列化"),
        )
        .expect("保存性能记录");
    }
}
