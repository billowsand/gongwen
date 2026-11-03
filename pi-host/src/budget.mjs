//! 工作预算与策略选择。
//!
//! 真实服务窗口（`contextWindow`）与「这次任务能放多少材料」是两个字段，不能靠把
//! `contextWindow` 改小来假装预算。这里只做保守估算：汉字按 1 token，其余按 4 字符
//! 1 token，再加 10% 余量；不引入分词器、不联网。策略依据任务规模与窗口，不按模型
//! 名称、参数量或某个固定窗口阈值猜能力。

/** 固定指令、技能索引与三条红线的预留。 */
export const INSTRUCTION_RESERVE = 5_000;
/** 安全余量。 */
const SAFETY_RESERVE = 2_048;
/** 预算不足以放下这些时明确失败，而不是把输出压到无法完成工作。 */
const MIN_OUTPUT_RESERVE = 1_024;

export function estimateTokens(text) {
  const source = typeof text === "string" ? text : "";
  let wide = 0;
  let narrow = 0;
  for (const char of source) {
    const code = char.codePointAt(0) ?? 0;
    if (code >= 0x2e80 || (code >= 0x2000 && code <= 0x206f)) wide += 1;
    else narrow += 1;
  }
  return Math.ceil((wide + narrow / 4) * 1.1);
}

function clamp(value, min, max) {
  return Math.min(max, Math.max(min, value));
}

/**
 * 任务规模估算。材料只拿已落盘的字节数做保守上界（UTF-8 汉字 3 字节），不读全文，
 * 免得在决策阶段就把大材料读进内存。
 */
export function taskEstimate(config) {
  const task = config.task ?? {};
  const materials = Array.isArray(task.materials) ? task.materials : [];
  const materialTokens = materials.reduce((sum, material) => {
    const bytes = Number.isInteger(material?.bytes) && material.bytes > 0 ? material.bytes : 0;
    return sum + Math.ceil(bytes / 2);
  }, 0);
  const requestTokens = estimateTokens(task.request ?? "") + estimateTokens(JSON.stringify(task.writingContext ?? {}));
  const currentBytes = Number.isInteger(config.currentBytes) ? config.currentBytes : 0;
  const currentTokens = Math.ceil(currentBytes / 2);
  const selectionTokens = estimateTokens(task.selection?.text ?? "");
  const report = isReportTask(task);
  const expectedOutput = report
    ? 12_000
    : clamp(2_000 + Math.ceil(currentTokens * 1.2), 2_000, 16_000);
  return {
    requestTokens,
    currentTokens,
    selectionTokens,
    materialTokens,
    materialCount: materials.length,
    factMaterialCount: materials.filter((material) =>
      material?.role === "fact" || material?.role === "incoming_letter").length,
    report,
    selection: Boolean(task.selection),
    expectedOutput,
    inputTokens: requestTokens + currentTokens + selectionTokens + materialTokens,
  };
}

/** 报告型任务：只交侧栏清单，不生成正文。 */
export function isReportTask(task) {
  const skill = typeof task?.skill === "string" ? task.skill : "";
  return skill === "gongwen-extract" || skill === "gongwen-style-learn";
}

/**
 * 计算预算与策略。返回的两个窗口字段始终分开：
 * - `window`：服务实际窗口（可能来自手动设置、服务报告或名称估计）；
 * - `workBudget`：本次任务依据的经验证 / 保守工作预算，不超过实际窗口。
 */
export function decideStrategy(config, estimate = taskEstimate(config)) {
  const window = Math.max(1, Number(config.model?.contextWindow) || 1);
  const capability = config.strategy?.capability ?? {};
  const verified =
    Number.isInteger(capability.verifiedWorkBudget) && capability.verifiedWorkBudget > 0
      ? Math.min(capability.verifiedWorkBudget, window)
      : undefined;
  const workBudget = verified ?? window;
  const maxTokens = Math.max(1, Number(config.model?.maxTokens) || 1);
  const outputReserve = Math.min(
    maxTokens,
    clamp(Math.floor(workBudget / 4), MIN_OUTPUT_RESERVE, 16_384),
  );
  const toolReserve = clamp(Math.floor(workBudget / 8), 2_048, 12_000);
  const materialsBudget = workBudget - INSTRUCTION_RESERVE - outputReserve - toolReserve - SAFETY_RESERVE;
  const reasons = [];
  const preference = config.strategy?.preference === "phased" ? "phased" : "auto";
  if (typeof capability.reason === "string" && capability.reason.trim()) {
    reasons.push(`能力档案：${capability.reason.slice(0, 500)}`);
  }

  if (materialsBudget >= 1_024 && (estimate.selection || (estimate.materialCount === 0 && estimate.requestTokens + estimate.currentTokens < 400))) {
    reasons.push("局部修改或很小的任务，按常规一轮完成");
    return {
      mode: "regular",
      preference,
      reasons,
      window,
      workBudget,
      materialsBudget,
      outputReserve,
      toolReserve,
      verified: Boolean(verified),
      estimate,
      fits: true,
      toolBudget: 60,
    };
  }

  if (materialsBudget < 1_024) {
    reasons.push("本次可用预算放不下最小指令、一次工具结果和有效输出");
    return {
      mode: "insufficient",
      preference,
      reasons,
      window,
      workBudget,
      materialsBudget,
      outputReserve,
      toolReserve,
      verified: Boolean(verified),
      estimate,
      fits: false,
      toolBudget: 0,
    };
  }

  const largeMaterials = estimate.materialTokens > materialsBudget * 0.9;
  const largeTask = estimate.inputTokens > materialsBudget * 0.9;
  const complex = estimate.materialCount > 0 || estimate.inputTokens > 6_000 || estimate.report;

  let phased = false;
  if (verified) reasons.push("工作预算来自已验证记录");
  else reasons.push("按实际窗口规划，优先完整阅读材料（不代表长上下文质量已验证）");

  if (preference === "phased" && complex) {
    phased = true;
    reasons.push("用户选择倾向分阶段");
  }
  if (largeMaterials) {
    phased = true;
    reasons.push("材料估算超过本次可用预算，需要分批提炼");
  }
  if (largeTask) {
    phased = true;
    reasons.push("正文、要求与材料的估算接近本次可用预算");
  }

  if (!phased) {
    reasons.push("任务规模在当前预算内，按常规一轮完成");
  } else if (estimate.report) {
    reasons.push("提炼任务只做材料提炼与清单交付，不套长稿阶段");
  } else if (estimate.selection) {
    reasons.push("局部修改不走长稿阶段");
  }

  return {
    mode: phased ? "phased" : "regular",
    preference,
    reasons,
    window,
    workBudget,
    materialsBudget,
    outputReserve,
    toolReserve,
    verified: Boolean(verified),
    estimate,
    fits: true,
    toolBudget: phased ? 72 : 60,
  };
}

/** 给界面的两行摘要：「分阶段 · 262k 窗口 / 96k 工作预算」。 */
export function budgetLabel(decision) {
  const window = decision.window >= 1024
    ? `${Math.round(decision.window / 1024)}k`
    : String(decision.window);
  const work = decision.workBudget >= 1024
    ? `${Math.round(decision.workBudget / 1024)}k`
    : String(decision.workBudget);
  return `${window} 窗口 / ${work} 工作预算`;
}

/** 单次工具结果能放多少字，由预算决定，不写死在读取工具里。 */
export function readBudgetChars(decision) {
  return clamp(Math.floor(decision.materialsBudget * 0.65), 512, 240_000);
}

/** 材料分块的字数上界。 */
export function chunkChars(decision) {
  return clamp(Math.floor(decision.materialsBudget * 0.55), 3_000, 160_000);
}
