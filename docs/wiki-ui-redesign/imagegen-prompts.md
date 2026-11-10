# 知识库设计图生成提示词

使用内置 ImageGen。每个画面使用共用提示词及对应页面提示词。后续修订以各画面的初稿为输入。

## 共用提示词

Use case: ui-mockup. Asset type: single high fidelity native desktop software screenshot for the Chinese offline document app 公文助手. Flat front-on edge-to-edge landscape 3:2 screenshot at 1536x1024, no device mockup, browser chrome, perspective, watermark or decorative marketing copy. Warm neutral light theme, warm off-white chrome #F4F2EC, paper #FFFFFF, charcoal ink #242923, thin gray-green borders #DDDCD4, restrained forest green #315C4E for active and primary, pale green selection, amber only for awaiting review, restrained red for missing or rejected. Match native egui desktop density, compact square-ish controls with 4px corners, clear alignment, readable Chinese system sans like PingFang SC, document headings Songti SC, one 1px divider between panes, no shadows except subtle edge around paper, no rounded cards everywhere. Fixed full-width app bar 公文助手  |  智能写作  资料库  知识库 then a slim route toolbar. App bar current nav must be underlined. Use a coordinated safety-production dataset throughout: 安全生产 (theme), 监督检查 and 责任体系 (categories), 重点隐患闭环整改 (wiki article), 2026 年度安全生产检查计划 and 安全生产责任制实施办法 (source documents). Give priority to correct exact Simplified Chinese labels and generous readable sizes, short realistic Chinese excerpts only. Visually clean, dense enough for daily professional use, strongly distinct pane roles.

## 01-browse

Wiki browsing, active nav 知识库. Show prototype’s complete three-pane layout. Route toolbar 知识库 with search box 搜索条目 then 框架管理, 缺口清单 and primary 待审收件箱 3. Left 240px persistent 分类框架 tree using disclosure chevrons, clear indentation, small guide/page icons, show 安全生产 expanded, 监督检查 expanded with 导览 and selected 重点隐患闭环整改, 责任体系, 政务服务. Center large dominant scrolling white document paper with breadcrumb 安全生产 / 监督检查, title 重点隐患闭环整改, small v3 · 版本历史, short sections 整改流程 and 复查销号, inline green underline hyperlink 责任体系, subdued red inline link 跨部门移交, inline clickable source tokens 〔E12〕 and 〔E18〕. Right 290px inspector, tabs 被引用 and 依据, 被引用 active, count 2 and two inbound article titles with one short context line each, collapse control at top. No AI task is running on this screen. Keep the entire screenshot visually like a calm well-designed editorial research desktop tool.

## 02-framework

Wiki framework compilation, active nav 知识库. Route toolbar ‹ 知识库 / 框架管理, tabs 从资料归纳 and 导入大纲, buttons 新增主题 and disabled 确认框架. Under toolbar a full-height workspace with editable hierarchical proposed tree on left 270px and central selected node details for 监督检查: name field, parent 安全生产, concise explanation, matching source list 2026 年度安全生产检查计划 and 安全生产责任制实施办法, clearly labeled 归纳依据, candidate preview tag 候选. Keep control row plus move up/down subtle. RIGHTMOST 335px dedicated always-visible AI work status pane structurally similar to a coding or writing assistant activity log, header AI 工作状态, activity title 归纳分类框架, pill 运行中 and elapsed 00:42. Vertical chronological timeline: 准备资料 · 已完成, 检索与阅读 · 已完成, 归纳主题与分类 · 进行中 with expanded useful row 已读 12 / 18 篇, 生成框架提案 · 等待. Footer detailed latest action 正在核对「监督检查」的归纳依据, expand activity and 停止. No chat bubbles or decorative progress ring. Pane extends from toolbar to bottom, with an actual vertical divider. User can inspect work in the center and knows human confirmation follows.

## 03-sources

Original source document organization and wiki compilation from selected materials, active nav 资料库. Toolbar 资料库 with local search 搜索原始资料, compact tabs 全部 / 未整编 / 已用于维基, 导入资料, selected count 已选 3 篇 and 圈选整编. Center-left wide native data table (roughly two thirds of width) with checkboxes, exact headings 文档, 文种, 维基状态, 关联条目. Three rows selected and six rows visible; selected sources include 2026 年度安全生产检查计划 and 安全生产责任制实施办法, status 未整编 and 已用于维基 as appropriate, article suggestions in plain link text. Below table, inline compact scope strip 整编范围 / 安全生产 › 监督检查 and three selected source titles or a concise file summary. RIGHTMOST 335px full-height AI pane, same structure as framework screen: AI 工作状态, activity 从圈选资料整编页面, 运行中 and elapsed 01:08, thin row 3 篇原始资料 · 监督检查. Vertical activity timeline 确认范围 ✓, 读取原文 ✓, 撰写条目 (current; expanded card 正在生成「重点隐患闭环整改」 and 可展开查看工具结果), 校验依据 (queued), 送交人工审核 (queued). Bottom action 停止. Show a distinct workspace for actual source organization, with data table taking precedence over AI rail.

## 04-inbox

Human review inbox, active nav 知识库. Route toolbar ‹ 知识库 / 待审收件箱, small tabs 待审核 3 and 已处理. Main composition is full-height master/detail rather than modal. Left 270px queue labeled 待审批次 with one selected batch 监督检查整编 and 3 页待审, another past batch optional. Center wide queue table with exact headers 页面 / 类型 / 依据 / 状态, three item rows 重点隐患闭环整改 (更新 · 2 篇 · 待审核), 检查频次 (新建 · 1 篇 · 待审核), 监督检查导览 (更新 · 2 篇 · 待审核); first row selected. Bottom or below table selected item overview with old version v3 and brief proposed change, a primary 打开审核台 action. RIGHTMOST full-height 335px AI WORK STATUS pane with exact header AI 工作状态, title 从圈选资料整编页面, subdued 已完成 pill and duration 01:34. Compact completed timeline 读取原文 · 3 篇, 撰写条目 · 3 页, 校验依据 · 通过, 送交人工审核 · 3 页. Bottom line 等待人工确认 and toggle 查看运行记录. Clear distinction between AI completed, pages still awaiting human decisions. Avoid giant cards and empty decoration.

## 05-review

Human review and confirmation desk, active nav 知识库. Make the user's requested page editing and evidence verification the center of attention. Route toolbar ‹ 待审收件箱 / 重点隐患闭环整改, status 更新 · 基线 v3, buttons 版式对照 and 变更对比, 1 / 3 previous/next. NO left tree or list on this route, so reading width is preserved. Under toolbar three full-height columns: left ~570px Markdown 源码, right next to it ~600px 版式预览, far right ~325px inspector. Source editor monospaced Chinese lines with line numbers, markdown headings and green emphasis around a short changed sentence, exact source token 〔E12〕. Preview looks like a native clean white Chinese document paper with matching title and short passage. Far-right inspector tabs 依据 and AI 工作状态, evidence active; expandable 〔E12〕 2026 年度安全生产检查计划 with short quoted original and path 第三节 · 复查销号, 〔E18〕 安全生产责任制实施办法 secondary row. Slim bottom toolbar spanning full screen, 左 返回收件箱, right 退回 and solid green 采纳并入库. Tiny line 已编辑 · 待确认 in center. Keep the two text surfaces genuinely spacious and visually aligned. Precision and legibility, professional government document review tool.

## 06-history

Wiki version history, active nav 知识库. Route toolbar ‹ 重点隐患闭环整改 / 版本历史. Left version timeline 250px, selectable v4 当前版本, v3, v2, with short change cause and approved dates. Center wide comparison view header 版本对比 with baseline selectors v3 → v4 and compact 变更 / 全文 toggle. Large readable side-by-side text diff with paired columns v3 and v4, only two changed sentences lightly tinted red and green with thin markers, realistic Chinese text under 复查销号. Right 290px inspector 版本详情, v4, 人工采纳, 本次新增依据 〔E12〕 2026 年度安全生产检查计划 and link 查看原文. Footer contextual secondary button 回滚到 v3 and small helper 回滚将生成新版本. Keep old and new content widths balanced, no active AI on this historical screen.

## 局部修订

### 01-browse

Edit this native Chinese desktop UI screenshot. Preserve its layout, dimensions, fonts, theme, app chrome, left tree and center document. Change only the content under the active right tab 被引用. Its two rows must show actual WIKI ARTICLES, not source documents. Use exactly these two linked titles, with a small article icon: 「监督检查导览」 and 「责任分工」. Below each title show one restrained short context snippet respectively 「……整改要求见重点隐患闭环整改。」 and 「……复查销号按重点隐患闭环整改办理。」. Keep 依据 as the inactive tab. Make no other changes.

### 02-framework

Edit this native Chinese desktop UI screenshot. Preserve the overall 3-column layout, dimensions, colors, app chrome, form evidence table and complete right AI work status timeline. Fix the framework tree on the left so it has exactly two levels: expanded theme 「安全生产」 has child categories 「监督检查」 selected, 「责任体系」 and 「应急管理」, and root theme 「政务服务」 is collapsed. Category rows are leaves without disclosure triangles or further children. Label tree header 「框架提案」 and selected form fields 「分类名称」, 「所属主题」, 「归纳说明」. Change source table rightmost column to 「支持要点」 with neutral short descriptions 「检查安排」 and 「职责分工」 instead of numerical or qualitative scores. Remove extra bottom middle buttons for saving, deleting or editing lower levels. In their place show subdued text 「框架提案完成后可编辑」. Keep disabled 「确认框架」 in the top toolbar. Keep all major layout and content outside these corrections intact.

### 03-sources

Use image 1 as edit target. Keep same layout, desktop chrome, columns and right activity panel. Make only these copy and status fixes. Use a gray neutral badge for every 「未整编」 row, not red. In row 5 replace 「待审核」 with a neutral 「未整编」. Below the toolbar replace the green enabled 「圈选整编」 button with a subdued disabled gray 「整编中」 control while the job is running. Under the queued right rail step 「校验依据」 replace the explanatory sentence with exactly 「检查引用编号和资料是否存在」. Preserve all other areas including the layout, widths, stop button and exact title 「AI 工作状态」.

### 04-inbox

Use image 1 as edit target. Keep all positions, size, palette, desktop chrome, three-pane layout, and lower evidence/change preview intact. Make only these precise labels consistent with an UPDATE batch. Rename selected batch 「监督检查更新」, its source 「新资料触发」, and the top right task title 「依据新资料更新页面」. Set all 3 rows in the 类型 column to 「更新」 including 检查频次. Under right rail 「撰写条目 · 3 页」 use exactly 「已生成 3 页更新提案。」. Under 「校验依据 · 通过」 use exactly 「引用编号与资料状态检查通过。」. Keep all three item status badges 「待审核」, the AI state 「已完成」, and bottom action 「打开审核台」 intact.
