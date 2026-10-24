//! 本次运行的词表撤销栈：一次保存对应一次撤销，批量修改也只占一笔。
use std::collections::VecDeque;

const MAX_BYTES: usize = 32 * 1024 * 1024;
const MAX_RECORDS: usize = 20;

#[derive(Default)]
pub(super) struct UndoStack {
    records: VecDeque<(Vec<u8>, String)>,
    bytes: usize,
}

impl UndoStack {
    pub fn push(&mut self, snapshot: Vec<u8>, label: String) {
        self.bytes += snapshot.len();
        self.records.push_back((snapshot, label));
        // 至少留最近一次；单份快照的大小已受个人文件读入上限约束。
        while self.records.len() > 1 && (self.records.len() > MAX_RECORDS || self.bytes > MAX_BYTES)
        {
            if let Some((bytes, _)) = self.records.pop_front() {
                self.bytes -= bytes.len();
            }
        }
    }

    pub fn last(&self) -> Option<&(Vec<u8>, String)> {
        self.records.back()
    }

    pub fn pop(&mut self) {
        if let Some((bytes, _)) = self.records.pop_back() {
            self.bytes -= bytes.len();
        }
    }

    pub fn clear(&mut self) {
        self.records.clear();
        self.bytes = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_snapshots_and_keeps_latest_operation() {
        let mut stack = UndoStack::default();
        for i in 0..30 {
            stack.push(vec![0; 10], i.to_string());
        }
        assert_eq!(stack.records.len(), MAX_RECORDS);
        assert_eq!(stack.last().unwrap().1, "29");
        stack.pop();
        assert_eq!(stack.last().unwrap().1, "28");
        stack.clear();
        stack.push(vec![0; MAX_BYTES], "大词表".into());
        stack.push(vec![0; 10], "最近一次".into());
        assert_eq!(stack.records.len(), 1);
        assert_eq!(stack.bytes, 10);
    }
}
