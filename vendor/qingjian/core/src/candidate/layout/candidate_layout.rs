use super::super::{Candidate, CandidateKind};
use super::Cell;

/// 本地候选的分页排布。索引空间是「格」：固定位置的候选占各自的格，其余依次填空。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CandidateLayout {
    /// 本地候选，顺序就是 Engine 排好的顺序。
    local: Vec<Candidate>,

    /// 每页几格。
    page_size: usize,
}

impl CandidateLayout {
    pub fn new(local: Vec<Candidate>, page_size: usize) -> Self {
        Self {
            local,
            page_size: page_size.max(1),
        }
    }

    pub fn page_size(&self) -> usize {
        self.page_size
    }

    pub fn local(&self) -> &[Candidate] {
        &self.local
    }

    /// 本地格数包含最大固定位置之前的空格。
    fn local_len(&self) -> usize {
        self.local
            .iter()
            .filter_map(|c| match c.kind {
                CandidateKind::Custom(position) => Some(position),
                _ => None,
            })
            .max()
            .unwrap_or(0)
            .max(self.local.len())
    }

    /// 真实本地候选按固定位置放置，其余候选依次填入空格。
    fn local_cells(&self) -> Vec<Cell<'_>> {
        let mut cells = vec![Cell::Empty; self.local_len()];
        for candidate in &self.local {
            if let CandidateKind::Custom(position) = candidate.kind
                && let Some(cell) = position.checked_sub(1).and_then(|i| cells.get_mut(i))
            {
                *cell = Cell::Local(candidate);
            }
        }
        let mut normal = self
            .local
            .iter()
            .filter(|c| !matches!(c.kind, CandidateKind::Custom(_)));
        for cell in &mut cells {
            if matches!(cell, Cell::Empty)
                && let Some(candidate) = normal.next()
            {
                *cell = Cell::Local(candidate);
            }
        }
        cells
    }

    /// 全部格子按索引顺序排开；空位只属于布局。
    pub fn cells(&self) -> Vec<Cell<'_>> {
        self.local_cells()
    }

    pub fn len(&self) -> usize {
        self.local_len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn pages(&self) -> usize {
        self.len().div_ceil(self.page_size)
    }

    /// 第 `index` 格的候选；越界返回 `None`。
    pub fn candidate(&self, index: usize) -> Option<&Candidate> {
        self.cells().get(index).and_then(|cell| cell.candidate())
    }

    /// 第 `page` 页的格子。
    pub fn page(&self, page: usize) -> Vec<Cell<'_>> {
        self.cells()
            .into_iter()
            .skip(page * self.page_size)
            .take(self.page_size)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(text: &str) -> Candidate {
        Candidate {
            text: text.into(),
            kind: CandidateKind::Chinese,
            syllables: vec!["zhang".into(), "tao".into()],
            reading: None,
            translation: None,
            fuma: None,
        }
    }

    fn texts(cells: &[Cell<'_>]) -> Vec<String> {
        cells
            .iter()
            .map(|cell| match cell {
                Cell::Local(c) => c.text.clone(),
                Cell::Empty => "<empty>".into(),
            })
            .collect()
    }

    fn many(count: usize) -> Vec<Candidate> {
        (0..count).map(|i| local(&format!("本{i}"))).collect()
    }

    #[test]
    fn candidates_fill_pages_in_order() {
        let layout = CandidateLayout::new(many(12), 9);
        assert_eq!(
            texts(&layout.page(0)),
            [
                "本0", "本1", "本2", "本3", "本4", "本5", "本6", "本7", "本8"
            ]
        );
        assert_eq!(texts(&layout.page(1)), ["本9", "本10", "本11"]);
        assert_eq!(layout.pages(), 2);
        assert_eq!(layout.candidate(9).unwrap().text, "本9");
    }

    #[test]
    fn sparse_custom_positions_are_layout_cells_not_candidates() {
        let fixed = Candidate {
            kind: CandidateKind::Custom(3),
            ..local("短语")
        };
        let layout = CandidateLayout::new(vec![fixed, local("普通")], 5);
        assert_eq!(layout.local().len(), 2);
        assert_eq!(texts(&layout.page(0)), ["普通", "<empty>", "短语"]);
        assert!(layout.candidate(1).is_none());
    }

    #[test]
    fn ninth_position_stays_on_second_page() {
        let layout = CandidateLayout::new(
            vec![Candidate {
                kind: CandidateKind::Custom(9),
                ..local("第九")
            }],
            5,
        );
        assert_eq!(layout.pages(), 2);
        assert_eq!(layout.candidate(8).unwrap().text, "第九");
        assert!(layout.candidate(7).is_none());
    }
}
