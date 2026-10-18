//! 公文词表分类、词条与首次建库状态。
//!
//! 由 src/models.rs 拆分而来：本文件是模块 `models::vocabulary`，与其它子模块共享
//! `models` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use serde::{Deserialize, Serialize};

/// 词库只有两种节点：单位是树干，人员是挂在单位下面的末端叶子。
/// 机关代字、简称、是否代章都是单位自身的属性，不再单列分类；
/// 会议地点与规范用语已从词库中移除（前者改为直接填写，后者不再影响成稿）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
pub enum VocabularyCategory {
    #[default]
    Unit,
    Person,
}

impl VocabularyCategory {
    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::Unit => "单位",
            Self::Person => "人员",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct VocabularyEntry {
    /// 词条稳定标识。树形界面用它记录展开与选中状态，重排序、改名都不受影响。
    /// `0` 表示尚未分配，载入词库时统一补号。
    pub id: u64,
    pub category: VocabularyCategory,
    /// 单位专属：层级编码，每级两位，下级以上级编码为前缀（`00` → `0001`）。
    /// 由树形结构自动生成；同级显示顺序由 `sort_order` 独立维护。
    pub code: String,
    pub canonical: String,
    /// 单位专属：对外行文时使用的名称；为空时回退正常名称并由审核给出提醒。
    pub external_name: String,
    pub aliases: Vec<String>,
    pub note: String,
    /// 与联系人绑定的电话；只对 `VocabularyCategory::Person` 有意义。
    pub phone: String,
    /// 人员专属：职务。姓名与职务分开维护，便于白头件按职务合并呈报称谓。
    pub position: String,
    /// 单位专属：上级单位的层级编码，空串表示顶层单位。
    /// 旧配置中可能仍是规范名称，载入后由 `units::normalize` 自动迁移。
    pub parent: String,
    /// 单位专属：本单位简称（如“新舆处”），空串时回落规范名称。
    pub abbr: String,
    /// 人员专属：所属单位的层级编码，用于“人随事走”过滤。
    /// 旧配置中可能仍是规范名称，载入后由 `units::normalize` 自动迁移。
    pub unit: String,
    /// 人员专属：允许该人员在公函版记中担任所属单位各级上级单位的承办联系人。
    pub can_handle_parent_unit: bool,
    /// 单位专属：发函代字。保留旧字段名以兼容已有词库，公函选中发文单位时自动带出。
    pub department_code: String,
    /// 单位专属：呈批代字，红头呈批件选中发文单位时自动带出。
    pub approval_department_code: String,
    /// 单位专属：职能任务（主要职责、分管事项）。智能体分配任务、推荐责任单位时按它对号。
    pub duties: String,
    /// 人员专属：个人简介（分管领域、专长、经历）。智能体推荐联系人、负责人时参考。
    pub profile: String,
    /// 单位专属：以该单位作为公函落款（联合发文时为主发文单位）时是否自动标注“（代章）”。
    /// 仅公函盖章；电话通知等其他文种不适用，一律不标注。
    pub seal_on_behalf: bool,
    /// 仅供 Markdown 导入合并判断该列是否真实存在；不写入本机配置。
    #[serde(skip)]
    pub(crate) seal_on_behalf_imported: bool,
    /// 同一上级下的显示顺序。层级编码负责表达上下级关系，排序值只负责界面、
    /// 下拉框与导出行的先后，二者分离后才能安全地在中间插入或拖拽排序。
    /// 旧配置没有该字段时为 0，载入整理时按原有顺序自动补齐。
    pub sort_order: u32,
}

/// 首次运行时标准词库的建库状态。单独持久化状态，避免用户已经完成建库后
/// 又清空词库时反复弹出新手引导。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum VocabularySetupStatus {
    #[default]
    Pending,
    Completed,
    Skipped,
}
