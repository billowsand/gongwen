//! 辅码（双拼辅助码）：触发、严格过滤、键消耗与学习。
//!
//! 样例词库只有 开发 / 开发者 / 开饭 / 开放 / 开 等词，表用它们：开 = `fk`、发 = `xa`。
//! 「开发」的双码期望是（开第 1 码 f, 发第 1 码 x）；单字「开」的期望是它自己的两码 (f, k)。

use super::*;

use crate::{FumaHint, FumaTable};

fn fuma_table() -> Arc<FumaTable> {
    Arc::new(FumaTable::parse("开=fk\n发=xa\n").unwrap())
}

fn fuma_engine() -> Engine {
    let mut engine = xiaohe();
    engine.set_fuma(Some(fuma_table()));
    engine
}

#[test]
fn fuma_filters_candidates_strictly() {
    let mut engine = fuma_engine();
    // 末 2 键含大写才激活：`fX` → (f, x)，正是（开第 1 码, 发第 1 码）
    engine.set_input("kdfafX");
    let items = &engine.query().unwrap().candidates.items;
    assert_eq!(items[0].text, "开发");
    // 对不上一个候选都不剩
    engine.set_input("kdfafY");
    assert!(engine.query().unwrap().candidates.items.is_empty());
    // 第一键大写是反转顺序：`Xf` → 交换成 (f, x)，同样对上
    engine.set_input("kdfaXf");
    assert_eq!(engine.query().unwrap().candidates.items[0].text, "开发");
    // 全小写不激活：末 2 键仍是双拼，前缀候选照常在（开 也在）
    engine.set_input("kdfafa");
    let items = &engine.query().unwrap().candidates.items;
    assert!(items.iter().any(|c| c.text == "开"));
    assert!(items.iter().any(|c| c.text == "开发"));
}

#[test]
fn fuma_filters_single_char_by_both_codes() {
    let mut engine = fuma_engine();
    // 单字期望本字两码：开 = (f, k)
    engine.set_input("kdfK");
    let items = &engine.query().unwrap().candidates.items;
    assert!(!items.is_empty());
    assert!(items.iter().all(|c| c.text == "开"));
    // 反转输入：`Kf` → 交换成 (f, k)
    engine.set_input("kdKf");
    assert!(
        engine
            .query()
            .unwrap()
            .candidates
            .items
            .iter()
            .all(|c| c.text == "开")
    );
    // 对不上就严格不剩
    engine.set_input("kdXk");
    assert!(engine.query().unwrap().candidates.items.is_empty());
    // 单键大写不激活（3 键是奇数）：末键按声母继续
    engine.set_input("kdN");
    assert_eq!(engine.query().unwrap().marked_text(), "kai'n");
}

#[test]
fn fuma_needs_a_complete_shuangpin_prefix() {
    let mut engine = fuma_engine();
    // 前缀解不完整（`nih` 落单 h）：末尾大写字母按普通拼音处理
    engine.set_input("nihKz");
    assert_eq!(engine.query().unwrap().marked_text(), "ni'huai'z");
    // 不足 4 键也不激活
    engine.set_input("nzK");
    let query = engine.query().unwrap();
    assert_eq!(query.marked_text(), "nou'k");
}

#[test]
fn fuma_commit_consumes_the_codes_too() {
    let mut engine = fuma_engine();
    engine.set_input("kdfafX");
    let query = engine.query().unwrap();
    let candidate = query.candidates.items[0].clone();
    assert_eq!(candidate.text, "开发");
    engine.commit(&candidate);
    // 6 个键全部吃掉，缓冲区清空
    assert!(engine.composition().is_empty());
    // 学习键不含辅码：记的是 kaifa
    assert!(engine.recent_commits.last().unwrap().same_input("kaifa"));

    // 没激活辅码时部分消耗照旧
    engine.set_input("kdfa");
    let query = engine.query().unwrap();
    let kai = query
        .candidates
        .items
        .iter()
        .find(|c| c.text == "开")
        .cloned()
        .unwrap();
    engine.commit(&kai);
    assert_eq!(engine.composition().text(), "fa");
}

#[test]
fn fuma_codes_go_away_with_a_prefix_candidate() {
    let mut engine = fuma_engine();
    // `fK` → (f, k)，正是单字「开」的两码；「开」只盖住前一个音节 kai
    engine.set_input("kdfafK");
    let items = engine.query().unwrap().candidates.items;
    assert!(items.iter().all(|c| c.text == "开"));
    engine.commit(&items[0]);
    // 辅码键随这次上屏一起丢掉：剩下的拼音不再背着它，否则 `fa` 又被筛一遍
    assert_eq!(engine.composition().text(), "fa");
}

/// 只敲首码（小写单键）：它还可能是下一个字的声母，两种读法都留着，辅码对得上的顶到最前。
#[test]
fn first_code_boosts_without_excluding() {
    let mut engine = fuma_engine();
    // 开=fk、发=xa。`kdf` 读成 `kai f…`：开发 / 开饭 / 开放 / 开发者 都是 f 声母的简拼词。
    // 同时 f 是 开（fk）与 开发（f + x）的辅码首码，这两条被顶到最前，其余照常留在后面
    engine.set_input("kdf");
    let items = &engine.query().unwrap().candidates.items;
    let texts: Vec<&str> = items.iter().map(|c| c.text.as_str()).collect();
    assert_eq!(&texts[..2], &["开发", "开"], "辅码首码对得上的排最前");
    // 首末字不在表里的（开饭 / 开放 / 开发者）没被排除，只是排在后面
    assert!(texts[2..].contains(&"开放"), "不排他：{texts:?}");
}

/// 两码那档（切不成音节的一对小写键）没有歧义，严格过滤。
#[test]
fn both_codes_filter_even_in_lower_case() {
    let mut engine = fuma_engine();
    // 开=fk：`fk` 解不成一个音节，是辅码
    engine.set_input("kdfk");
    let items = &engine.query().unwrap().candidates.items;
    assert!(!items.is_empty());
    assert!(items.iter().all(|c| c.text == "开"));
    // 两键正好是一个合法音节时不抢：`fa` 是 fa，`kdfa` 照常读成 kai'fa
    engine.set_input("kdfa");
    assert_eq!(engine.query().unwrap().marked_text(), "kai'fa");
}

/// 只敲了首码：首码对得上的候选右上角标还要敲的第二码，对不上的不标。
#[test]
fn first_code_marks_the_remaining_code() {
    let mut engine = fuma_engine();
    engine.set_input("kdf");
    let items = &engine.query().unwrap().candidates.items;
    let kai = items.iter().find(|c| c.text == "开").unwrap();
    assert_eq!(kai.fuma.as_deref(), Some("k"));
    // 词组取首字第 1 码 + 末字第 1 码：开发 = fx，首码 f 对上，还要敲 x
    let kaifa = items.iter().find(|c| c.text == "开发").unwrap();
    assert_eq!(kaifa.fuma.as_deref(), Some("x"));
    // 首码对不上的一律不标（开 = fk、开发 = fx，首码都是 f）
    engine.set_input("kdx");
    let items = &engine.query().unwrap().candidates.items;
    assert!(items.iter().all(|c| c.fuma.is_none()));
    // 两码敲满：已严格过滤，辅码段在拼音行里，不再标
    engine.set_input("kdfK");
    let items = &engine.query().unwrap().candidates.items;
    assert!(!items.is_empty());
    assert!(items.iter().all(|c| c.fuma.is_none()));
    // 缺省档没敲辅码时不标
    engine.set_input("kd");
    let items = &engine.query().unwrap().candidates.items;
    assert!(items.iter().all(|c| c.fuma.is_none()));
}

/// 学码档：没敲辅码时标完整两码；敲了首码仍只标第二码；关掉就什么都不标。
#[test]
fn fuma_hint_levels() {
    let mut engine = fuma_engine();
    engine.set_fuma_hint(FumaHint::Always);
    engine.set_input("kd");
    let items = &engine.query().unwrap().candidates.items;
    let kai = items.iter().find(|c| c.text == "开").unwrap();
    assert_eq!(kai.fuma.as_deref(), Some("fk"));
    engine.set_input("kdfa");
    let items = &engine.query().unwrap().candidates.items;
    let kaifa = items.iter().find(|c| c.text == "开发").unwrap();
    assert_eq!(kaifa.fuma.as_deref(), Some("fx"));
    engine.set_input("kdf");
    let items = &engine.query().unwrap().candidates.items;
    assert_eq!(
        items
            .iter()
            .find(|c| c.text == "开")
            .unwrap()
            .fuma
            .as_deref(),
        Some("k")
    );

    engine.set_fuma_hint(FumaHint::Off);
    for input in ["kd", "kdf"] {
        engine.set_input(input);
        let items = &engine.query().unwrap().candidates.items;
        assert!(items.iter().all(|c| c.fuma.is_none()), "{input}");
    }

    // 没开辅码表时学码档也不标
    let mut plain = xiaohe();
    plain.set_fuma_hint(FumaHint::Always);
    plain.set_input("kd");
    let items = &plain.query().unwrap().candidates.items;
    assert!(items.iter().all(|c| c.fuma.is_none()));
}

/// 首码那档选中辅码候选时，那一键跟着一起吃掉；选普通前缀候选时它留着当下一个字的声母。
#[test]
fn first_code_is_eaten_only_by_its_own_candidate() {
    let mut engine = fuma_engine();
    engine.set_input("kdf");
    let kai = engine
        .query()
        .unwrap()
        .candidates
        .items
        .iter()
        .find(|c| c.text == "开")
        .cloned()
        .unwrap();
    engine.commit(&kai);
    // 开 的首码就是 f：连辅码键一起吃光
    assert!(engine.composition().is_empty());

    // 同样的输入选 开发（盖满 `kai f…` 的简拼词）：按拼音算，也是全吃
    engine.set_input("kdf");
    let kaifa = engine
        .query()
        .unwrap()
        .candidates
        .items
        .iter()
        .find(|c| c.text == "开发")
        .cloned()
        .unwrap();
    engine.commit(&kaifa);
    assert!(engine.composition().is_empty());
}

#[test]
fn fuma_shows_up_in_the_pinyin_line() {
    let mut engine = fuma_engine();
    // 激活时辅码段单独一段跟在拼音后面，原样保留大小写：敲的是 `fX` 就显示 `fX`
    engine.set_input("kdfafX");
    let query = engine.query().unwrap();
    assert_eq!(query.marked_text(), "kai'fa fX");
    let segments = query.marked_segments();
    assert_eq!(segments.len(), 2);
    assert_eq!(segments[0].kind, MarkedKind::Typed);
    assert_eq!(segments[1].kind, MarkedKind::Fuma);
    assert_eq!(segments[1].text, " fX");
    // 光标算在辅码段之后（用户正在末尾敲）
    assert_eq!(query.marked_cursor(), "kai'fa fX".chars().count());

    // 反转写法照样原样显示
    engine.set_input("kdfaXf");
    assert_eq!(engine.query().unwrap().marked_text(), "kai'fa Xf");

    // 没激活时拼音行不变，一个字都不多
    engine.set_input("kdfafa");
    let query = engine.query().unwrap();
    assert_eq!(query.marked_text(), "kai'fa'fa");
    assert!(query.fuma.is_none());
    assert!(
        query
            .marked_segments()
            .iter()
            .all(|s| s.kind != MarkedKind::Fuma)
    );
}

#[test]
fn fuma_raw_commit_keeps_every_key() {
    let mut engine = fuma_engine();
    engine.set_input("kdfafY");
    assert!(engine.query().unwrap().candidates.items.is_empty());
    // 回车上屏敲的键本身，辅码段也在：没候选时回车多半是要打英文
    assert_eq!(engine.take_raw(), "kdfafY");
    // `rust` 被读成 ru + 辅码 st，筛空了；回车要上屏整个 rust，不是 ru
    engine.set_input("rust");
    assert_eq!(engine.take_raw(), "rust");
    // 全小写时（未激活）原样上屏
    engine.set_input("kdfafa");
    assert_eq!(engine.take_raw(), "kdfafa");
}

#[test]
fn fuma_codes_leave_a_whole_english_word_in_the_candidates() {
    let words = WordList::parse("rust\trust\t3740\n").unwrap();
    let mut engine = fuma_engine().with_english(words);
    // `st` 解不成音节，被当成 ru 的辅码筛光了中文候选；整串是英文词，照出
    engine.set_input("rust");
    let items = &engine.query().unwrap().candidates.items;
    assert_eq!(items[0].text, "rust");
    assert_eq!(items[0].kind, CandidateKind::English);
    // 辅码筛出了字时，筛出的字仍在第一
    engine.set_input("kdfafX");
    assert_eq!(engine.query().unwrap().candidates.items[0].text, "开发");
}

#[test]
fn fuma_backspace_deletes_codes_keywise() {
    let mut engine = fuma_engine();
    engine.set_input("kdfafX");
    // 末尾一对辅码键一起删
    assert!(engine.delete_syllable_backward());
    assert_eq!(engine.composition().text(), "kdfa");
    // 再删就是音节对
    assert!(engine.delete_syllable_backward());
    assert_eq!(engine.composition().text(), "kd");
    // 只敲了一个辅码键（未激活）单删
    engine.set_input("kdfaf");
    assert!(engine.delete_syllable_backward());
    assert_eq!(engine.composition().text(), "kdfa");
}

#[test]
fn fuma_without_table_or_full_pinyin_changes_nothing() {
    // 没有表：大写字母不是拼音键，照旧当英文直输
    let mut plain_xiaohe = xiaohe();
    plain_xiaohe.set_input("kdfaFX");
    assert!(plain_xiaohe.raw_mode());
    // 辅码只在双拼下生效：全拼 + 表不激活，大写字母照旧英文直输
    let mut plain = engine();
    plain.set_fuma(Some(fuma_table()));
    plain.set_input("kaifafX");
    assert!(plain.raw_mode());
    let query = plain.query().unwrap();
    assert_eq!(query.candidates.items[0].text, "kaifafX");
}

/// 两码正好也是个音节（`wody`：沃 = `dy`，`dy` 又是 dun）：辅码优先，两码全对的前缀候选顶到最前，
/// 选中时连这两键一起吃掉；按两个音节读的候选照常留着，选它们时这两键留作下一个音节。
#[test]
fn syllable_shaped_codes_take_priority_without_excluding() {
    // 这张表里 开 = `fa`，正好是音节 fa 的双拼
    let mut engine = xiaohe();
    engine.set_fuma(Some(Arc::new(FumaTable::parse("开=fa\n发=xa\n").unwrap())));
    engine.set_input("kdfa");
    let query = engine.query().unwrap();
    // 拼音行照旧读成两个音节，那两键不剥
    assert_eq!(query.marked_text(), "kai'fa");
    let texts: Vec<&str> = query
        .candidates
        .items
        .iter()
        .map(|c| c.text.as_str())
        .collect();
    assert_eq!(texts[0], "开", "两码全对的排最前：{texts:?}");
    assert!(texts.contains(&"开发"), "不排他：{texts:?}");
    // 两码已经敲满，不再标还要敲的码
    assert!(query.candidates.items[0].fuma.is_none());

    // 选 开：辅码键一起吃掉，不留成下一个音节
    let kai = query.candidates.items[0].clone();
    engine.commit(&kai);
    assert!(engine.composition().is_empty());

    // 选按两个音节读的 开发：同样全吃
    engine.set_input("kdfa");
    let kaifa = engine
        .query()
        .unwrap()
        .candidates
        .items
        .iter()
        .find(|c| c.text == "开发")
        .cloned()
        .unwrap();
    engine.commit(&kaifa);
    assert!(engine.composition().is_empty());
}

/// 两码对不上时一切照旧：没有候选被顶上来，选前缀候选时那两键就是下一个音节。
#[test]
fn syllable_shaped_codes_that_miss_change_nothing() {
    // 默认表里 开 = `fk`，`fa` 对不上
    let mut engine = fuma_engine();
    engine.set_input("kdfa");
    let items = engine.query().unwrap().candidates.items;
    assert_eq!(items[0].text, "开发");
    let kai = items.iter().find(|c| c.text == "开").cloned().unwrap();
    engine.commit(&kai);
    assert_eq!(engine.composition().text(), "fa");
}
