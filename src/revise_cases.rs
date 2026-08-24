//! 文字复核回归集：离线校验闸门，接上模型后算召回率与误报率。
//!
//! 为什么要有这个：调阈值和改提示词都是「改一处、不知道有没有弄坏别处」的活。
//! 没有一份固定的样例集，每次改动都只能凭手感，改到第三次就说不清是变好了还是
//! 变差了。
//!
//! 集子分两半，作用完全不同：
//!
//! - **「该报」的病句**衡量检查器灵不灵；
//! - **「不该报」的正常句**衡量它烦不烦人——这一半才是真正的门槛。一个天天误报
//!   的检查器，用户不会去逐条忽略，会直接把整个功能关掉。
//!
//! 对应地，测试也分两半：
//!
//! - **离线**（进 CI）：每条「该报」样例的参考改法都必须通过闸门。这一条不需要
//!   任何模型，却挡住了一类很隐蔽的退化——**闸门太严把正确的修改也拦下**。它和
//!   「检查器太笨」一样糟，区别只在于它不会报错，只会静悄悄什么都不报。
//! - **在线**（`#[ignore]`，本机手动跑）：拿真模型跑一遍，报召回率与误报率。
//!
//! 样例文件不含任何真实公文内容，全部按公文语体手写，可以随仓库分发；各单位可
//! 自行追加本单位行文习惯的样例，格式相同。
//!
//! 标注本身也会错，而且错得比模型更隐蔽。首轮实测报出的两条「漏报」，查下来是
//! 标注把行文风格当成了语病（「认真研究并抓紧予以解决」的停顿、「把 X 作为 Y
//! 来抓」的「来」），模型不报才是对的，已改判为负样例。**看到漏报先怀疑标注，
//! 再怀疑模型**——否则会照着错标注去调提示词，把本来正确的行为调坏。

use crate::revise_model::{self, GateReason};

const CASES: &str = include_str!("../revise-cases.tsv");

/// 一条样例。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Case {
    pub id: String,
    pub kind: String,
    /// 期望检查器报不报这一句。
    pub should_flag: bool,
    /// 该由哪个检查器报。负样例留空——它的要求更强：**任何**检查器都不该报。
    pub task: String,
    pub sentence: String,
    /// 期望的改法；`should_flag` 为假时为空。
    pub expected: String,
    pub note: String,
}

/// 解析样例文件。格式错误直接 panic：这是编译进二进制的固定资产，
/// 出问题属于打包事故，不该在运行期悄悄少跑几条。
pub fn cases() -> Vec<Case> {
    let mut out = Vec::new();
    let mut header_seen = false;
    for (index, line) in CASES.lines().enumerate() {
        let line = line.trim_end();
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        if !header_seen {
            header_seen = true;
            assert_eq!(
                cols.first().map(|c| c.trim()),
                Some("编号"),
                "回归集第 {} 行不是预期的表头",
                index + 1
            );
            continue;
        }
        assert!(
            cols.len() >= 7,
            "回归集第 {} 行只有 {} 列，需要 7 列",
            index + 1,
            cols.len()
        );
        let should_flag = match cols[2].trim() {
            "该报" => true,
            "不该报" => false,
            other => panic!("回归集第 {} 行的期望「{other}」无法识别", index + 1),
        };
        let task = cols[3].trim().to_string();
        assert!(
            task.is_empty() || revise_model::task_by_id(&task).is_some(),
            "回归集第 {} 行的检查器「{task}」不存在",
            index + 1
        );
        out.push(Case {
            id: cols[0].trim().to_string(),
            kind: cols[1].trim().to_string(),
            should_flag,
            task,
            sentence: cols[4].trim().to_string(),
            expected: cols[5].trim().to_string(),
            note: cols[6].trim().to_string(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proofread::Lexicon;

    fn lexicon() -> Lexicon {
        Lexicon::resolved(&Default::default())
    }

    #[test]
    fn the_case_file_parses_and_is_balanced() {
        let cases = cases();
        assert!(cases.len() >= 30, "回归集太小，说明不了问题");
        let flagged = cases.iter().filter(|case| case.should_flag).count();
        let clean = cases.len() - flagged;
        // 负样本少于三成时，误报率算出来没有意义——而误报正是这个检查器
        // 会不会被用户关掉的决定因素。
        assert!(
            clean * 10 >= cases.len() * 3,
            "「不该报」样例只有 {clean} 条，占比过低"
        );
        let mut ids: Vec<&str> = cases.iter().map(|case| case.id.as_str()).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(before, ids.len(), "回归集存在重复编号");
    }

    #[test]
    fn flagged_cases_carry_a_reference_fix_and_clean_ones_do_not() {
        for case in cases() {
            if case.should_flag {
                assert!(
                    !case.expected.is_empty(),
                    "{} 标为该报却没给参考改法，离线测试就无从验证闸门",
                    case.id
                );
                assert_ne!(
                    case.sentence, case.expected,
                    "{} 的参考改法与原句相同",
                    case.id
                );
                assert!(
                    !case.task.is_empty(),
                    "{} 标为该报却没写归属哪个检查器",
                    case.id
                );
            } else {
                assert!(case.expected.is_empty(), "{} 标为不该报却给了改法", case.id);
                assert!(
                    case.task.is_empty(),
                    "{} 是负样例，不该限定检查器——任何检查器报了它都算误报",
                    case.id
                );
            }
        }
    }

    /// 每条样例都必须真的会被送进模型。被切分规则过滤掉的样例是空跑的——
    /// 它永远「通过」，却什么都没验证。
    #[test]
    fn every_case_survives_sentence_segmentation() {
        let limit = crate::models::ReviseModelConfig::default().max_sentence_chars;
        for case in cases() {
            let sentences = revise_model::segment_sentences(&case.sentence, limit);
            assert_eq!(
                sentences.len(),
                1,
                "{} 没有被切成恰好一句（{} 句），这条样例是空跑的",
                case.id,
                sentences.len()
            );
            assert_eq!(
                sentences[0].text, case.sentence,
                "{} 切分后与原句不一致",
                case.id
            );
        }
    }

    /// 闸门不能把正确的修改也拦下。
    ///
    /// 这是整个离线部分的核心：闸门太严和检查器太笨后果一样——什么都不报——
    /// 但前者不会报错，只会静悄悄地退化，除非有这么一条测试盯着。
    #[test]
    fn the_gate_lets_every_reference_fix_through() {
        let lexicon = lexicon();
        for case in cases().into_iter().filter(|case| case.should_flag) {
            let result = revise_model::gate(&lexicon, &[], &case.sentence, &case.expected);
            assert!(
                result.is_ok(),
                "{} 的参考改法被闸门以「{}」拦下：{} → {}",
                case.id,
                result.unwrap_err().label(),
                case.sentence,
                case.expected
            );
        }
    }

    /// 参考改法必须能被还原成一次最小替换，且替换回去与整句一致。
    /// 算不出区间的改法，就算模型给对了也落不了地。
    #[test]
    fn every_reference_fix_reduces_to_one_applicable_edit() {
        for case in cases().into_iter().filter(|case| case.should_flag) {
            let (span, replacement) = revise_model::minimal_edit(&case.sentence, &case.expected)
                .unwrap_or_else(|| panic!("{} 的参考改法算不出改动区间", case.id));
            assert!(!span.is_empty(), "{} 的改动区间为空，锚不住", case.id);
            let mut applied = case.sentence.clone();
            applied.replace_range(span, &replacement);
            assert_eq!(
                applied, case.expected,
                "{} 的最小替换拼不回参考改法",
                case.id
            );
        }
    }

    /// 模型跑飞的几种典型形态都必须被闸门按正确的类别拦下。
    ///
    /// 每条各带自己的原句，而不是共用一句：闸门是**按顺序**判定的，事实排在
    /// 裸数字之前。要验证「裸数字」这一档，原句里就不能有单位名——否则往里插
    /// 一个数字会把单位名割开，事实闸门先一步拦下，验的就不是想验的那条了。
    /// 这一点是这条测试自己撞出来的。
    #[test]
    fn the_gate_catches_each_way_a_model_goes_wrong() {
        let lexicon = lexicon();
        let report = "请于本月底前将落实情况书面报送我办综合科。";
        let audit = "本次抽查共覆盖第 3 类事项，均已按期完成。";
        let expectations = [
            (report, "", GateReason::Shape),
            (
                report,
                "请于本月底前报送。\n另外建议补充说明。",
                GateReason::Shape,
            ),
            (
                report,
                "请各有关单位务必于本月底之前，将本单位落实上述各项要求的具体情况，形成书面材料后报送我办综合科汇总。",
                GateReason::Length,
            ),
            // 用真日期，不用「本月底 → 下月底」：后者不是提取器认得的日期格式，
            // 词库为空时精确版本来就拦不住它。原先它能被拦下，靠的是贪婪单位
            // 正则误打误撞——那种保护是假的，不该写进测试当成真的。
            (
                "请于2026年8月21日前将落实情况报送我办综合科。",
                "请于2026年8月22日前将落实情况报送我办综合科。",
                GateReason::Facts,
            ),
            (
                audit,
                "本次抽查共覆盖第 5 类事项，均已按期完成。",
                GateReason::Digits,
            ),
            (
                report,
                "请于本月底前将**落实情况**书面报送我办综合科。",
                GateReason::Markup,
            ),
            (
                report,
                "请于本月底前将落实情况书面报送我办综合科并布署后续工作。",
                GateReason::NewTypo,
            ),
        ];
        for (base, rewritten, expected) in expectations {
            assert_eq!(
                revise_model::gate(&lexicon, &[], base, rewritten),
                Err(expected),
                "改写「{rewritten}」应当按「{}」拦下",
                expected.label()
            );
        }
    }

    /// 改动词库里的单位名必须按「改动关键事实」拦下。
    ///
    /// 词库是权威且精确的，这条保护不能因为上面放宽了正则兜底而一起松掉。
    #[test]
    fn changing_a_vocabulary_unit_is_a_fact_change() {
        let vocabulary = vec![crate::models::VocabularyEntry {
            category: crate::models::VocabularyCategory::Unit,
            canonical: "市财政局".into(),
            ..Default::default()
        }];
        assert_eq!(
            revise_model::gate(
                &lexicon(),
                &vocabulary,
                "请市财政局于本月底前反馈意见。",
                "请市教育局于本月底前反馈意见。",
            ),
            Err(GateReason::Facts),
        );
    }

    /// 句中含单位名时，正常的语病修改不得被事实闸门误伤。
    ///
    /// 这是本轮真正抓到的缺陷：`ai_guard` 的词库外单位兜底正则是贪婪的，
    /// 「请于本月底前将落实情况书面报送我办综合科」整串会被当成一个单位名，
    /// 于是句首任何改动都算「改了单位」。公文句子里到处是局、处、科、中心，
    /// 这等于把绝大多数正常修改无声丢掉——无声是最糟的部分，它不报错。
    ///
    /// 回归集里原有的 24 条正样例一条都不含单位名，正是这个盲区让缺陷藏住了。
    #[test]
    fn a_grammar_fix_survives_even_when_the_sentence_names_a_unit() {
        assert!(
            revise_model::gate(
                &lexicon(),
                &[],
                "通过市财政局的大力支持，使项目建设进度明显加快。",
                "市财政局的大力支持使项目建设进度明显加快。",
            )
            .is_ok(),
            "含单位名的句子里，正常的语病修改不该被拦下"
        );
    }

    /// 拿真模型跑一遍回归集，报召回率与误报率。
    ///
    /// 不进 CI：它要连本机的模型服务，几十次推理，结果也不是稳定的通过/失败，
    /// 而是两个需要人来看的比率。
    ///
    /// 跑法（先在设置里配好复核模型）：
    ///
    /// ```text
    /// cargo test --bin gongwen-assistant revise_cases -- --ignored --nocapture
    /// ```
    ///
    /// **改提示词之前请先跑多遍**，否则量到的多半是随机性：
    ///
    /// ```text
    /// GONGWEN_EVAL_REPEATS=3 cargo test --bin gongwen-assistant revise_cases \
    ///     -- --ignored --nocapture
    /// ```
    ///
    /// 温度 0 不等于确定性——批处理、GPU 归约顺序、KV 缓存都会让同一输入得出
    /// 不同结果。跑一遍时 0/28 和 1/28 分不开，而这两个数会导向完全相反的决定。
    #[test]
    #[ignore = "需要本机模型服务才能运行"]
    fn measure_recall_and_false_alarms_against_a_live_model() {
        let config = crate::storage::load().unwrap_or_default();
        let cfg = config.revise_model.clone();
        let resolved = cfg.resolve(&config.lm_studio);
        // 报错要说清楚「去哪儿改、改成什么」。只说「请在设置中配置」，用户还得
        // 自己去找配置文件在哪——尤其配置目录在各平台上并不一样。
        assert!(
            !resolved.model.trim().is_empty(),
            "文字复核还没有选模型，无法评测。二选一：\n\
             （1）打开应用 →「设置 → AI 文字复核」→ 勾选启用 → 选一个模型；\n\
             （2）直接编辑 {}\n\
             \u{20}   把 revise_model.model 填成模型名（如 qwen3-8b），\n\
             \u{20}   base_url 留空则沿用起草模型的 {}。\n\
             另外请确认本机模型服务已启动并加载了该模型。",
            crate::storage::config_path()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|error| format!("（配置文件路径不可用：{error:#}）")),
            if config.lm_studio.base_url.trim().is_empty() {
                "（起草模型也没填地址）".to_string()
            } else {
                config.lm_studio.base_url.clone()
            },
        );
        let lexicon = lexicon();
        let cases = cases();
        // 同一句跑几遍。温度 0 并不保证确定性（批处理、GPU 归约顺序、KV 缓存都会
        // 让同一输入得出不同结果），单跑一遍的 1/28 和 0/28 根本分不开。
        let repeats: usize = std::env::var("GONGWEN_EVAL_REPEATS")
            .ok()
            .and_then(|value| value.parse().ok())
            .filter(|value| *value > 0)
            .unwrap_or(1);

        for task in &revise_model::TASKS {
            // 每条样例记「报了几次 / 跑了几次」。
            let mut flagged_runs: Vec<(&Case, usize)> = Vec::new();
            let mut rejected_by = std::collections::BTreeMap::<&str, usize>::new();

            for case in &cases {
                // 正样例只由它归属的检查器负责；负样例每个检查器都要过——
                // 「不该报」的要求是任何一个都不许报。
                if case.should_flag && case.task != task.id {
                    continue;
                }
                let mut flagged = 0usize;
                let mut sample = None;
                for _ in 0..repeats {
                    let outcome = revise_model::review(
                        revise_model::ReviewRequest {
                            cfg: &cfg,
                            draft_model: &config.lm_studio,
                            lexicon: &lexicon,
                            vocabulary: &config.vocabulary,
                            markdown: &case.sentence,
                            cache: &Default::default(),
                            tasks: &[task],
                        },
                        &|_, _| {},
                    )
                    .expect("复核调用失败");
                    for ((_, reason), count) in &outcome.rejected_by_reason {
                        *rejected_by.entry(reason.label()).or_default() += *count as usize;
                    }
                    if let Some(first) = outcome.suggestions.first() {
                        flagged += 1;
                        sample.get_or_insert_with(|| {
                            format!("把「{}」改成「{}」", first.before, first.after)
                        });
                    }
                }
                if case.should_flag && flagged < repeats {
                    println!(
                        "[{}] 漏报 {}（{flagged}/{repeats} 次报出）：{}",
                        task.id, case.id, case.sentence
                    );
                }
                if !case.should_flag && flagged > 0 {
                    println!(
                        "[{}] 误报 {}（{flagged}/{repeats} 次报出）：{} → 建议{}",
                        task.id,
                        case.id,
                        case.sentence,
                        sample.as_deref().unwrap_or("（无）")
                    );
                }
                flagged_runs.push((case, flagged));
            }

            report_task(task, &flagged_runs, repeats, &rejected_by);
        }

        println!(
            "\n以上数字不含任何稿件内容，可以直接贴出来讨论。\n\
             判读：**先看区间宽不宽**。区间跨了好几个百分点说明结果被随机性主导，\
             此时改提示词是在追噪音——加大 GONGWEN_EVAL_REPEATS 重跑，或先扩样例。\n\
             区间收窄之后：误报高就单独关掉那一项或补 exclusions；召回低多半是模型\
             偏小。看到漏报先核对标注——标注错了照着调只会越调越坏。"
        );
    }

    /// 把一个检查器的结果打成「下界 ~ 上界」。
    ///
    /// 只报一个数会把随机性当成事实。下界按「每次都报」算，上界按「至少报一次」
    /// 算：两者相等说明结果稳定，可以据此下判断；两者一拉开就说明这一轮量到的
    /// 主要是噪音，不该拿去调参。
    fn report_task(
        task: &revise_model::ReviseTask,
        rows: &[(&Case, usize)],
        repeats: usize,
        rejected_by: &std::collections::BTreeMap<&str, usize>,
    ) {
        let mut always_hit = 0usize;
        let mut ever_hit = 0usize;
        let mut positives = 0usize;
        let mut always_alarm = 0usize;
        let mut ever_alarm = 0usize;
        let mut negatives = 0usize;
        let mut unstable = 0usize;
        for (case, flagged) in rows {
            if *flagged > 0 && *flagged < repeats {
                unstable += 1;
            }
            if case.should_flag {
                positives += 1;
                if *flagged == repeats {
                    always_hit += 1;
                }
                if *flagged > 0 {
                    ever_hit += 1;
                }
            } else {
                negatives += 1;
                if *flagged == repeats {
                    always_alarm += 1;
                }
                if *flagged > 0 {
                    ever_alarm += 1;
                }
            }
        }
        println!("\n== {}（{}）· 每句跑 {repeats} 遍 ==", task.label, task.id);
        println!(
            "召回率：{always_hit}~{ever_hit} / {positives} = {:.0}%~{:.0}%",
            percent(always_hit, positives),
            percent(ever_hit, positives)
        );
        println!(
            "误报率：{always_alarm}~{ever_alarm} / {negatives} = {:.0}%~{:.0}%",
            percent(always_alarm, negatives),
            percent(ever_alarm, negatives)
        );
        if unstable > 0 {
            println!("其中 {unstable} 条时报时不报——这些是随机性，不是能力");
        }
        if !rejected_by.is_empty() {
            let parts: Vec<String> = rejected_by
                .iter()
                .map(|(reason, count)| format!("{reason} {count}"))
                .collect();
            println!("闸门拦截：{}", parts.join("、"));
        }
    }

    fn percent(part: usize, total: usize) -> f64 {
        if total == 0 {
            return 0.0;
        }
        part as f64 * 100.0 / total as f64
    }
}
