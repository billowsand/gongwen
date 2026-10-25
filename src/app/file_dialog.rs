//! 系统文件对话框：对话框开在后台线程，主线程不阻塞。
//!
//! Linux/Wayland 下合成器每 1.5 秒给客户端发一次存活探测，客户端的事件循环
//! 停摆超过阈值就被判成「应用无响应」，Hyprland 会弹出系统对话框。旧实现用
//! `rfd::FileDialog` 的阻塞接口在主线程上等用户选文件——挑一个文件本来就要
//! 几十秒，主线程一停，探测没人回，系统对话框就弹了出来；应用本身却照常能
//! 用，看上去只是多了个系统弹窗。
//!
//! 现在统一走 [`GongwenApp::request_file`] / [`GongwenApp::request_files`]：
//! 对话框在后台线程开，主线程每帧问一次结果，用户挑完再接着做原来那件事。
//! 只 portal 后端（Linux/Wayland 发布包）走后台线程：GTK3 后端的对话框是本
//! 进程自己的窗口，必须留在主线程。

use std::path::PathBuf;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};

use super::GongwenApp;

/// 对话框的后台结果：用户挑中的路径（取消时是 `None`）。
type FileDialogFuture = Pin<Box<dyn Future<Output = Option<Vec<PathBuf>>>>>;

/// 对话框要做什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileDialogKind {
    /// 挑一个文件。
    PickFile,
    /// 挑多个文件。
    PickFiles,
    /// 挑一个目录。
    PickFolder,
    /// 挑一个保存位置。
    SaveFile,
}

/// 一次文件对话框的请求参数。
#[derive(Debug, Clone)]
pub(crate) struct FileDialogSpec {
    kind: FileDialogKind,
    title: Option<String>,
    filters: Vec<(String, Vec<String>)>,
    file_name: Option<String>,
    directory: Option<PathBuf>,
}

impl FileDialogSpec {
    /// 挑一个文件。
    pub(crate) fn pick_file() -> Self {
        Self::of(FileDialogKind::PickFile)
    }

    /// 挑多个文件。
    pub(crate) fn pick_files() -> Self {
        Self::of(FileDialogKind::PickFiles)
    }

    /// 挑一个目录。
    pub(crate) fn pick_folder() -> Self {
        Self::of(FileDialogKind::PickFolder)
    }

    /// 挑一个保存位置。
    pub(crate) fn save_file() -> Self {
        Self::of(FileDialogKind::SaveFile)
    }

    fn of(kind: FileDialogKind) -> Self {
        Self {
            kind,
            title: None,
            filters: Vec::new(),
            file_name: None,
            directory: None,
        }
    }

    /// 对话框标题。
    pub(crate) fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// 加一个按扩展名过滤的分组。
    pub(crate) fn filter(mut self, name: impl Into<String>, extensions: &[&str]) -> Self {
        self.filters.push((
            name.into(),
            extensions.iter().map(|ext| (*ext).to_string()).collect(),
        ));
        self
    }

    /// 默认文件名（保存时用）。
    pub(crate) fn file_name(mut self, name: impl Into<String>) -> Self {
        self.file_name = Some(name.into());
        self
    }

    /// 对话框打开时的起始目录。
    pub(crate) fn directory(mut self, path: impl Into<PathBuf>) -> Self {
        self.directory = Some(path.into());
        self
    }
}

/// 用户挑完之后的回调：拿到对话框的结果。
type FileCallback<T> = Box<dyn FnOnce(&mut GongwenApp, T)>;

/// 用户挑完之后的回调：单选与多选各一种。
enum FileDialogCallback {
    One(FileCallback<Option<PathBuf>>),
    Many(FileCallback<Option<Vec<PathBuf>>>),
}

/// 一个还没问完结果的对话框。
pub(super) struct PendingFileDialog {
    future: FileDialogFuture,
    callback: FileDialogCallback,
}

impl GongwenApp {
    /// 挑一个文件。用户挑完（或取消）后回调拿到路径。
    ///
    /// 回调在主线程上跑，与原来「点了按钮当场选文件、接着往下做」是同一个
    /// 时序，只是中间隔了一次帧。
    pub(crate) fn request_file(
        &mut self,
        spec: FileDialogSpec,
        on_done: impl FnOnce(&mut GongwenApp, Option<PathBuf>) + 'static,
    ) {
        self.push_file_dialog(spec, FileDialogCallback::One(Box::new(on_done)));
    }

    /// 挑多个文件。用户挑完（或取消）后回调拿到路径清单。
    pub(crate) fn request_files(
        &mut self,
        spec: FileDialogSpec,
        on_done: impl FnOnce(&mut GongwenApp, Option<Vec<PathBuf>>) + 'static,
    ) {
        self.push_file_dialog(spec, FileDialogCallback::Many(Box::new(on_done)));
    }

    /// 每帧问一次还没问完的对话框。用户挑完就把结果交给回调。
    pub(crate) fn poll_file_dialogs(&mut self, ctx: &eframe::egui::Context) {
        if self.pending_file_dialogs.is_empty() {
            return;
        }
        // 后台线程的结果不会叫醒主线程，这里按固定节奏问一次。
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
        let mut finished = Vec::new();
        for (index, pending) in self.pending_file_dialogs.iter_mut().enumerate() {
            if let Some(result) = poll_pending(&mut pending.future) {
                finished.push((index, result));
            }
        }
        for (index, result) in finished.into_iter().rev() {
            let pending = self.pending_file_dialogs.remove(index);
            match pending.callback {
                FileDialogCallback::One(on_done) => {
                    on_done(self, result.and_then(|paths| paths.into_iter().next()));
                }
                FileDialogCallback::Many(on_done) => on_done(self, result),
            }
        }
    }

    /// 登记一个对话框，同一时间只开一个。
    fn push_file_dialog(&mut self, spec: FileDialogSpec, callback: FileDialogCallback) {
        if !self.pending_file_dialogs.is_empty() {
            self.status = "已有一个文件对话框开着，先把它关掉。".into();
            return;
        }
        self.pending_file_dialogs.push(PendingFileDialog {
            future: spec.start(),
            callback,
        });
    }
}

/// 问一次后台线程的结果：`Some` 表示问到了（可能是用户取消）。
fn poll_pending(future: &mut FileDialogFuture) -> Option<Option<Vec<PathBuf>>> {
    // 不用真唤醒：主线程按固定节奏自己来问。
    let mut context = Context::from_waker(Waker::noop());
    match future.as_mut().poll(&mut context) {
        Poll::Ready(result) => Some(result),
        Poll::Pending => None,
    }
}

impl FileDialogSpec {
    /// 开对话框，返回一个随时能问结果的任务。
    ///
    /// portal 后端（Linux/Wayland 变体）在后台线程开窗，主线程照常画界面、照常
    /// 回应合成器的存活探测；GTK3 后端的对话框是本进程自己的窗口，只能留在
    /// 主线程当场开，结果照样走同一条投递链。
    #[cfg(feature = "linux-portal-dialogs")]
    fn start(self) -> FileDialogFuture {
        let mut dialog = rfd::AsyncFileDialog::new();
        if let Some(title) = self.title {
            dialog = dialog.set_title(title);
        }
        for (name, extensions) in &self.filters {
            let extensions: Vec<&str> = extensions.iter().map(String::as_str).collect();
            dialog = dialog.add_filter(name, &extensions);
        }
        if let Some(file_name) = self.file_name {
            dialog = dialog.set_file_name(file_name);
        }
        if let Some(directory) = self.directory {
            dialog = dialog.set_directory(directory);
        }
        match self.kind {
            FileDialogKind::PickFile => Box::pin(async move {
                dialog
                    .pick_file()
                    .await
                    .map(|handle| vec![handle.path().to_path_buf()])
            }),
            FileDialogKind::PickFiles => Box::pin(async move {
                dialog.pick_files().await.map(|handles| {
                    handles
                        .iter()
                        .map(|handle| handle.path().to_path_buf())
                        .collect()
                })
            }),
            FileDialogKind::PickFolder => Box::pin(async move {
                dialog
                    .pick_folder()
                    .await
                    .map(|handle| vec![handle.path().to_path_buf()])
            }),
            FileDialogKind::SaveFile => Box::pin(async move {
                dialog
                    .save_file()
                    .await
                    .map(|handle| vec![handle.path().to_path_buf()])
            }),
        }
    }

    /// 当场开对话框并等结果，返回一个立刻就绪的任务。
    #[cfg(not(feature = "linux-portal-dialogs"))]
    fn start(self) -> FileDialogFuture {
        let mut dialog = rfd::FileDialog::new();
        if let Some(title) = self.title {
            dialog = dialog.set_title(title);
        }
        for (name, extensions) in &self.filters {
            let extensions: Vec<&str> = extensions.iter().map(String::as_str).collect();
            dialog = dialog.add_filter(name, &extensions);
        }
        if let Some(file_name) = self.file_name {
            dialog = dialog.set_file_name(file_name);
        }
        if let Some(directory) = self.directory {
            dialog = dialog.set_directory(directory);
        }
        let picked = match self.kind {
            FileDialogKind::PickFile => dialog.pick_file().map(|path| vec![path]),
            FileDialogKind::PickFiles => dialog.pick_files(),
            FileDialogKind::PickFolder => dialog.pick_folder().map(|path| vec![path]),
            FileDialogKind::SaveFile => dialog.save_file().map(|path| vec![path]),
        };
        Box::pin(async move { picked })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::management_samples;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// 用户挑完文件之后，原来的那件事要接着做，而且只做一次。
    #[test]
    fn picked_path_reaches_the_callback_exactly_once() {
        let mut app = management_samples::fixture();
        let ctx = eframe::egui::Context::default();
        let seen: Rc<RefCell<Vec<Option<PathBuf>>>> = Rc::new(RefCell::new(Vec::new()));
        let sink = seen.clone();
        app.pending_file_dialogs.push(PendingFileDialog {
            future: Box::pin(std::future::pending()),
            callback: FileDialogCallback::One(Box::new(move |_app, path| {
                sink.borrow_mut().push(path);
            })),
        });

        // 结果还没回来：回调不能跑，登记也不能丢。
        app.poll_file_dialogs(&ctx);
        assert!(seen.borrow().is_empty(), "结果没回来之前不能跑回调");
        assert_eq!(app.pending_file_dialogs.len(), 1);

        let picked = PathBuf::from("/tmp/公文.docx");
        let answer = picked.clone();
        app.pending_file_dialogs[0].future = Box::pin(async move { Some(vec![answer]) });
        app.poll_file_dialogs(&ctx);
        assert_eq!(seen.borrow().len(), 1, "回调只能跑一次");
        assert_eq!(seen.borrow()[0].as_deref(), Some(picked.as_path()));
        assert!(app.pending_file_dialogs.is_empty(), "问完的对话框要清掉");
    }

    /// 取消选择时回调也要收到结果：没有路径，原来的那件事不接着做。
    #[test]
    fn cancelled_dialog_reports_no_path() {
        let mut app = management_samples::fixture();
        let ctx = eframe::egui::Context::default();
        let seen: Rc<RefCell<Vec<Option<PathBuf>>>> = Rc::new(RefCell::new(Vec::new()));
        let sink = seen.clone();
        app.pending_file_dialogs.push(PendingFileDialog {
            future: Box::pin(async { None }),
            callback: FileDialogCallback::One(Box::new(move |_app, path| {
                sink.borrow_mut().push(path);
            })),
        });

        app.poll_file_dialogs(&ctx);
        assert_eq!(seen.borrow().len(), 1);
        assert!(seen.borrow()[0].is_none(), "取消就是没有路径");
    }
}
