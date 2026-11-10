//! 维基知识库存储层测试。全部走内存库，覆盖框架两级约束、页面版本链、
//! 链接解析（页面/节点/红链）、backlinks、删除保护、改名跟随、检索与资料标记。

use super::*;

fn store() -> WikiStore {
    WikiStore::open_in_memory().unwrap()
}

/// 造出「安全生产 → 监督检查」框架并返回 (theme_id, category_id)。
fn seed_frame(s: &WikiStore) -> (i64, i64) {
    let t = s.create_theme("安全生产").unwrap();
    let c = s.create_category(t, "监督检查").unwrap();
    (t, c)
}

/// 组一个建页入参。
fn page(title: &str, node_id: i64, kind: &'static str, body: &str) -> PageInput {
    PageInput {
        title: title.into(),
        node_id,
        kind,
        body: body.into(),
        source_run_id: String::new(),
        review_edited: false,
        doc_refs: Vec::new(),
        base_version: None,
    }
}

#[test]
fn frame_two_levels_enforced() {
    let s = store();
    let (t, c) = seed_frame(&s);
    // 分类下不能再建分类。
    assert!(s.create_category(c, "非法下级").is_err());
    // 挂到不存在的主题报错。
    assert!(s.create_category(9999, "幽灵").is_err());
    let tree = s.list_tree().unwrap();
    assert_eq!(tree.len(), 1);
    assert_eq!(tree[0].theme.name, "安全生产");
    assert_eq!(tree[0].categories.len(), 1);
    assert_eq!(tree[0].categories[0].id, c);
    let _ = t;
}

#[test]
fn node_delete_guards() {
    let s = store();
    let (t, c) = seed_frame(&s);
    // 空分类可删；主题下还有分类时主题删不掉。
    assert!(s.delete_node(t).is_err());
    s.delete_node(c).unwrap();
    s.delete_node(t).unwrap();
    // 有页面时节点删不掉。
    let t = s.create_theme("政务服务").unwrap();
    let c = s.create_category(t, "一网通办").unwrap();
    s.create_page(&page("一网通办", c, "normal", "正文"))
        .unwrap();
    assert!(s.delete_node(c).is_err());
    assert!(s.delete_node(t).is_err());
}

#[test]
fn node_rename_and_move_category() {
    let s = store();
    let (t1, c) = seed_frame(&s);
    let t2 = s.create_theme("政务服务").unwrap();
    // 改名后树里是新名。
    s.rename_node(c, "监督检查与执法").unwrap();
    let tree = s.list_tree().unwrap();
    assert_eq!(tree[0].categories[0].name, "监督检查与执法");
    // 分类可跨主题移动。
    s.move_category(c, t2).unwrap();
    let tree = s.list_tree().unwrap();
    let t1_node = tree.iter().find(|x| x.theme.id == t1).unwrap();
    let t2_node = tree.iter().find(|x| x.theme.id == t2).unwrap();
    assert!(t1_node.categories.is_empty());
    assert_eq!(t2_node.categories.len(), 1);
}

#[test]
fn create_page_resolves_links_and_backlinks() {
    let s = store();
    let (_, c) = seed_frame(&s);
    // 先建目标页，链接解析时它已存在 → 解析为页面链接。
    let b = s
        .create_page(&page("责任体系", c, "normal", "职责分工。"))
        .unwrap();
    let a = s
        .create_page(&page(
            "监督检查",
            c,
            "normal",
            "见 [[责任体系]] 与 [[安全生产]]。",
        ))
        .unwrap();
    // 目标框架节点存在 → 解析为节点链接。
    let links = s.outgoing_links(&a).unwrap();
    assert_eq!(links.len(), 2);
    let to_page = links.iter().find(|l| l.to_page.is_some()).unwrap();
    assert_eq!(to_page.to_page.as_deref(), Some(b.as_str()));
    assert!(to_page.resolved);
    let to_node = links.iter().find(|l| l.to_node.is_some()).unwrap();
    assert_eq!(to_node.to_node, Some(1)); // 第一个建的主题「安全生产」
    // backlinks：责任体系被「监督检查」引用。
    let bl = s.backlinks(&b).unwrap();
    assert_eq!(bl.len(), 1);
    assert_eq!(bl[0].from_title, "监督检查");
}

#[test]
fn red_link_unresolved() {
    let s = store();
    let (_, c) = seed_frame(&s);
    let a = s
        .create_page(&page("甲页", c, "normal", "将来补 [[安全文化]] 页。"))
        .unwrap();
    let links = s.outgoing_links(&a).unwrap();
    assert_eq!(links.len(), 1);
    assert!(!links[0].resolved);
    assert_eq!(links[0].to_title, "安全文化");
    assert_eq!(links[0].to_page, None);
}

#[test]
fn red_link_gaps_group_unresolved_targets() {
    let s = store();
    let (_, c) = seed_frame(&s);
    let a = s
        .create_page(&page("甲页", c, "normal", "见 [[安全文化]]。"))
        .unwrap();
    let b = s
        .create_page(&page(
            "乙页",
            c,
            "normal",
            "也见 [[安全文化]]，以及 [[责任体系]]。",
        ))
        .unwrap();
    let _ = (a, b);
    let gaps = s.red_link_gaps().unwrap();
    assert_eq!(gaps.len(), 2, "两个不同的未建目标");
    let culture = gaps.iter().find(|g| g.title == "安全文化").unwrap();
    assert_eq!(culture.refs.len(), 2, "同一目标按标题聚合");
    let duty = gaps.iter().find(|g| g.title == "责任体系").unwrap();
    assert_eq!(duty.refs.len(), 1);
    // 建页之后红链消失（链接按标题重新解析为页面）。
    s.create_page(&page("安全文化", c, "normal", "内容。"))
        .unwrap();
    let gaps = s.red_link_gaps().unwrap();
    assert_eq!(gaps.len(), 1);
    assert_eq!(gaps[0].title, "责任体系");
}

#[test]
fn duplicate_title_and_guide_rejected() {
    let s = store();
    let (_, c) = seed_frame(&s);
    s.create_page(&page("监督检查", c, "normal", "一")).unwrap();
    assert!(s.create_page(&page("监督检查", c, "normal", "二")).is_err());
    // 同节点只能有一篇导览页。
    s.create_page(&page("监督检查导览", c, "guide", "导览"))
        .unwrap();
    assert!(
        s.create_page(&page("另一导览", c, "guide", "导览"))
            .is_err()
    );
}

#[test]
fn list_pages_orders_guide_first() {
    let s = store();
    let (_, c) = seed_frame(&s);
    s.create_page(&page("监督检查", c, "normal", "一")).unwrap();
    s.create_page(&page("监督检查导览", c, "guide", "导览"))
        .unwrap();
    let pages = s.list_pages(c).unwrap();
    assert_eq!(pages.len(), 2);
    assert_eq!(pages[0].kind, "guide");
    assert_eq!(pages[1].title, "监督检查");
}

#[test]
fn update_creates_version_and_rebuilds_links() {
    let s = store();
    let (_, c) = seed_frame(&s);
    let b = s.create_page(&page("责任体系", c, "normal", "旧")).unwrap();
    let mut with_refs = page("监督检查", c, "normal", "见 [[责任体系]]。");
    s.conn.execute("INSERT INTO knowledge_docs(id, source, created_at, updated_at) VALUES (7, 'markdown', '', '')", []).unwrap();
    with_refs.doc_refs = vec![(7, "支撑要点".into())];
    let a = s.create_page(&with_refs).unwrap();
    // 依据标记派生：doc 7 被一页使用。
    let status = s.doc_wiki_status().unwrap();
    assert_eq!(status.get(&7), Some(&1));
    assert_eq!(s.page_doc_refs(&a).unwrap().len(), 1);

    let v2 = s
        .update_page(&a, &page("监督检查", c, "normal", "不再链接。"))
        .unwrap();
    assert_eq!(v2, 2);
    let page_now = s.get_page(&a).unwrap().unwrap();
    assert_eq!(page_now.meta.current_version, 2);
    assert_eq!(page_now.body, "不再链接。");
    // 旧链接被清掉。
    assert!(s.outgoing_links(&a).unwrap().is_empty());
    // 依据随新版本清空。
    assert!(s.page_doc_refs(&a).unwrap().is_empty());
    let status = s.doc_wiki_status().unwrap();
    assert!(!status.contains_key(&7));
    // 版本链：v1 正文仍在。
    assert_eq!(
        s.get_version_body(&a, 1).unwrap().as_deref(),
        Some("见 [[责任体系]]。")
    );
    let versions = s.list_versions(&a).unwrap();
    assert_eq!(versions.len(), 2);
    assert!(!versions[0].review_edited); // v2 未标人工
    assert!(!versions[1].review_edited);
    assert_eq!(s.version_doc_refs(&a, 1).unwrap(), with_refs.doc_refs);
    assert_eq!(s.rollback(&a, 1).unwrap(), 3);
    assert_eq!(s.page_doc_refs(&a).unwrap(), with_refs.doc_refs);
    let _ = b;
}

#[test]
fn review_edited_flag_recorded() {
    let s = store();
    let (_, c) = seed_frame(&s);
    let mut input = page("监督检查", c, "normal", "原文");
    input.review_edited = true;
    input.source_run_id = "run-42".into();
    let a = s.create_page(&input).unwrap();
    let versions = s.list_versions(&a).unwrap();
    assert!(versions[0].review_edited);
    assert_eq!(versions[0].source_run_id, "run-42");
}

#[test]
fn rollback_advances_version_chain() {
    let s = store();
    let (_, c) = seed_frame(&s);
    let a = s.create_page(&page("甲页", c, "normal", "第一版")).unwrap();
    s.update_page(&a, &page("甲页", c, "normal", "第二版"))
        .unwrap();
    let v3 = s.rollback(&a, 1).unwrap();
    assert_eq!(v3, 3);
    let page_now = s.get_page(&a).unwrap().unwrap();
    assert_eq!(page_now.body, "第一版");
    assert_eq!(s.list_versions(&a).unwrap().len(), 3);
}

#[test]
fn delete_page_protected_by_backlinks() {
    let s = store();
    let (_, c) = seed_frame(&s);
    let b = s.create_page(&page("责任体系", c, "normal", "旧")).unwrap();
    let a = s
        .create_page(&page("监督检查", c, "normal", "见 [[责任体系]]。"))
        .unwrap();
    // 被引用 → 拒删。
    assert!(s.delete_page(&b).is_err());
    // 删掉引用页后可以删。
    s.delete_page(&a).unwrap();
    s.delete_page(&b).unwrap();
}

#[test]
fn rename_keeps_links_by_uuid() {
    let s = store();
    let (_, c) = seed_frame(&s);
    let b = s.create_page(&page("责任体系", c, "normal", "旧")).unwrap();
    let a = s
        .create_page(&page("监督检查", c, "normal", "见 [[责任体系]]。"))
        .unwrap();
    // 改标题（链接按 uuid 解析，不受影响）。
    s.update_page(&b, &page("安全生产责任体系", c, "normal", "新"))
        .unwrap();
    let links = s.outgoing_links(&a).unwrap();
    assert_eq!(links[0].to_page.as_deref(), Some(b.as_str()));
    assert!(links[0].resolved);
    let bl = s.backlinks(&b).unwrap();
    assert_eq!(bl.len(), 1);
    // 新标题可被新链接解析。
    let c2 = s
        .create_page(&page("乙页", c, "normal", "见 [[安全生产责任体系]]。"))
        .unwrap();
    assert!(s.outgoing_links(&c2).unwrap()[0].resolved);
    let _ = a;
}

#[test]
fn search_hits_title_and_body() {
    let s = store();
    let (_, c) = seed_frame(&s);
    s.create_page(&page("监督检查", c, "normal", "四不两直与闭环整改。"))
        .unwrap();
    s.create_page(&page("责任体系", c, "normal", "职责分工与考核。"))
        .unwrap();
    let hits = s.search("闭环整改", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].title, "监督检查");
    let hits = s.search("责任", 10).unwrap();
    assert!(!hits.is_empty());
    assert!(s.search("", 10).unwrap().is_empty());
}

#[test]
fn parse_frame_outline_variants() {
    // 标题式与列表式等价；混排时列表分类可跟在标题主题后。
    let heading =
        parse_frame_outline("# 安全生产\n## 监督检查\n## 应急管理\n# 政务服务\n").unwrap();
    assert_eq!(heading.len(), 2);
    assert_eq!(heading[0].name, "安全生产");
    assert_eq!(heading[0].categories, vec!["监督检查", "应急管理"]);
    assert!(heading[1].categories.is_empty());
    let list = parse_frame_outline("- 安全生产\n  - 监督检查\n  - 应急管理\n- 政务服务\n").unwrap();
    assert_eq!(list, heading);
    let mixed = parse_frame_outline("# 安全生产\n  - 监督检查\n").unwrap();
    assert_eq!(mixed[0].categories, vec!["监督检查"]);

    assert!(parse_frame_outline("").is_err());
    assert!(parse_frame_outline("随便一行\n").is_err());
    assert!(parse_frame_outline("### 三级\n").is_err());
    assert!(parse_frame_outline("- 甲\n    - 太深\n").is_err());
    assert!(parse_frame_outline("## 孤儿分类\n").is_err());
    assert!(parse_frame_outline("# 重复\n# 重复\n").is_err());
    assert!(parse_frame_outline("# 甲\n- 乙\n- 乙\n").is_err());
    assert!(parse_frame_outline(&format!("# {}\n", "长".repeat(61))).is_err());
}

#[test]
fn parse_wikilinks_variants() {
    // [[ ]] 目标为空，跳过；其余按序解析。
    let links = parse_wikilinks("见 [[甲]]、[[乙|乙页面]] 与 [[ ]] 以及 [[丙]]。");
    assert_eq!(
        links,
        vec![
            RawLink {
                target: "甲".into(),
                text: "甲".into()
            },
            RawLink {
                target: "乙".into(),
                text: "乙页面".into()
            },
            RawLink {
                target: "丙".into(),
                text: "丙".into()
            },
        ]
    );
    // 未闭合不产出。
    assert!(parse_wikilinks("[[未闭合").is_empty());
}

#[test]
fn stale_update_cannot_overwrite_a_later_version() {
    let s = store();
    let (_, c) = seed_frame(&s);
    let uuid = s
        .create_page(&page("检查制度", c, "normal", "第一版"))
        .unwrap();
    let mut input = page("检查制度", c, "normal", "过时提案");
    input.base_version = Some(1);
    s.rollback(&uuid, 1).unwrap();
    assert!(
        s.update_page(&uuid, &input)
            .unwrap_err()
            .to_string()
            .contains("v2")
    );
    assert_eq!(s.list_versions(&uuid).unwrap().len(), 2);
}

#[test]
fn stored_link_survives_a_rename_and_an_edit_after_the_old_title_is_reused() {
    let s = store();
    let (_, c) = seed_frame(&s);
    let target = s.create_page(&page("旧名", c, "normal", "内容")).unwrap();
    let source = s
        .create_page(&page("源页", c, "normal", "见 [[旧名]]"))
        .unwrap();
    s.update_page(&target, &page("新名", c, "normal", "内容"))
        .unwrap();
    s.create_page(&page("旧名", c, "normal", "无关内容"))
        .unwrap();
    s.update_page(&source, &page("源页", c, "normal", "参见 [[旧名]]"))
        .unwrap();
    let links = s.link_bindings(&source, "参见 [[旧名]]").unwrap();
    assert_eq!(links["旧名"].to_page.as_deref(), Some(target.as_str()));
}

#[test]
fn guide_alias_protects_page_and_node_deletion_preserves_red_link() {
    let s = store();
    let (theme, category) = seed_frame(&s);
    let source = s
        .create_page(&page(
            "源页",
            category,
            "normal",
            "[[安全生产]] [[暂用分类]]",
        ))
        .unwrap();
    let guide = s
        .create_page(&page("安全生产总览", theme, "guide", "内容"))
        .unwrap();
    assert_eq!(s.backlinks(&guide).unwrap()[0].from_uuid, source);
    assert!(s.delete_page(&guide).is_err());
    let node = s.create_category(theme, "暂用分类").unwrap();
    s.update_page(&source, &page("源页", category, "normal", "[[安全生产]]"))
        .unwrap();
    s.update_page(
        &source,
        &page("源页", category, "normal", "[[安全生产]] [[暂用分类]]"),
    )
    .unwrap();
    s.delete_node(node).unwrap();
    assert_eq!(s.red_link_gaps().unwrap()[0].title, "暂用分类");
}

#[test]
fn frame_mapping_rolls_back_and_deleted_page_sources_remain_auditable() {
    let s = store();
    let proposal = FrameTheme {
        name: "主题".into(),
        categories: vec![FrameCategory {
            name: "分类".into(),
            doc_ids: vec![17],
        }],
    };
    assert!(s.apply_frame(std::slice::from_ref(&proposal)).is_err());
    assert!(s.list_tree().unwrap().is_empty());
    s.conn.execute("INSERT INTO knowledge_docs(id, source, created_at, updated_at) VALUES (17, 'markdown', '', '')", []).unwrap();
    assert_eq!(
        s.apply_frame(std::slice::from_ref(&proposal)).unwrap(),
        (2, 0)
    );
    assert_eq!(s.apply_frame(&[proposal]).unwrap(), (0, 2));
    let tree = s.list_tree().unwrap();
    let category = tree[0].categories[0].id;
    assert_eq!(s.frame_doc_ids(category).unwrap(), vec![17]);
    let mut input = page("条目", category, "normal", "依据见 〔E17〕");
    input.doc_refs = vec![(17, "说明".into())];
    let uuid = s.create_page(&input).unwrap();
    s.conn
        .execute("DELETE FROM knowledge_docs WHERE id=17", [])
        .unwrap();
    assert!(s.frame_doc_ids(category).unwrap().is_empty());
    assert!(s.update_page(&uuid, &input).is_err());
    assert_eq!(
        s.pages_needing_review().unwrap(),
        vec![(uuid.clone(), "条目".into())]
    );
    assert_eq!(s.version_doc_refs(&uuid, 1).unwrap(), input.doc_refs);
    assert!(s.page_has_missing_sources(&uuid).unwrap());
    assert_eq!(s.search("条目", 10).unwrap().len(), 1);
    assert!(s.search_for_evidence("条目", 10).unwrap().is_empty());
}
