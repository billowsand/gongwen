//! 按内容哈希缓存预览里每帧都要重做的纯计算（Markdown 解析、切块）。
//!
//! 预览每帧从头画一遍，正文没动时这些结果每帧都一样；长稿解析一次就要一毫秒
//! 以上，60 页的花脸稿约 1.2 ms，每次鼠标移动、滚动动画、光标闪烁都要付一遍。
//! 结果挂在 egui 的临时数据上，每类计算留最近几份：同一时刻可能有好几块预览
//! （版本对照右栏、稿件管理对照窗……）各画各的稿子，只留一份会互相挤掉。

use eframe::egui;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// 每类计算保留的份数。
const SLOTS: usize = 4;

/// 最近用过的几份结果，最近的在前。
struct Slots<T>(Vec<(u64, Arc<T>)>);

impl<T> Clone for Slots<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> Default for Slots<T> {
    fn default() -> Self {
        Self(Vec::new())
    }
}

/// 把若干输入合成一个缓存键。
pub(crate) fn key(parts: impl Hash) -> u64 {
    let mut hasher = std::hash::DefaultHasher::new();
    parts.hash(&mut hasher);
    hasher.finish()
}

/// 取 `slot` 这类计算里键为 `key` 的结果；没有就现算一份存起来。
/// `key` 必须覆盖 `compute` 用到的全部输入。
pub(crate) fn memo<T: Send + Sync + 'static>(
    ctx: &egui::Context,
    slot: &'static str,
    key: u64,
    compute: impl FnOnce() -> T,
) -> Arc<T> {
    let id = egui::Id::new(("gw-preview-memo", slot));
    let hit = ctx.data_mut(|data| {
        let slots = data.get_temp_mut_or_default::<Slots<T>>(id);
        let index = slots.0.iter().position(|(cached, _)| *cached == key)?;
        let entry = slots.0.remove(index);
        let value = entry.1.clone();
        slots.0.insert(0, entry);
        Some(value)
    });
    if let Some(value) = hit {
        return value;
    }
    // 算的时候不持有 egui 的锁：解析长稿要一两毫秒，期间别的线程可能要读上下文。
    let value = Arc::new(compute());
    ctx.data_mut(|data| {
        let slots = data.get_temp_mut_or_default::<Slots<T>>(id);
        slots.0.insert(0, (key, value.clone()));
        slots.0.truncate(SLOTS);
    });
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_key_reuses_the_result_and_old_entries_fall_off() {
        let ctx = egui::Context::default();
        let mut runs = 0;
        let first = memo(&ctx, "test", 1, || {
            runs += 1;
            "一".to_string()
        });
        let again = memo(&ctx, "test", 1, || {
            runs += 1;
            "不该算".to_string()
        });
        assert_eq!(runs, 1);
        assert!(Arc::ptr_eq(&first, &again));

        // 挤满之后最久没用的那份被丢掉，再取就得重算。
        for key in 2..=(SLOTS as u64 + 1) {
            memo(&ctx, "test", key, || key.to_string());
        }
        let recomputed = memo(&ctx, "test", 1, || {
            runs += 1;
            "重算".to_string()
        });
        assert_eq!(runs, 2);
        assert_eq!(recomputed.as_str(), "重算");
    }

    #[test]
    fn slots_of_different_kinds_do_not_collide() {
        let ctx = egui::Context::default();
        let text = memo(&ctx, "text", 7, || "字".to_string());
        let number = memo(&ctx, "number", 7, || 7usize);
        assert_eq!(text.as_str(), "字");
        assert_eq!(*number, 7);
    }
}
