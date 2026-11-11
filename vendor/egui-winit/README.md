# egui-winit

[![Latest version](https://img.shields.io/crates/v/egui-winit.svg)](https://crates.io/crates/egui-winit)
[![Documentation](https://docs.rs/egui-winit/badge.svg)](https://docs.rs/egui-winit)
![MIT](https://img.shields.io/badge/license-MIT-blue.svg)
![Apache](https://img.shields.io/badge/license-Apache-blue.svg)

This crates provides bindings between [`egui`](https://github.com/emilk/egui) and [`winit`](https://crates.io/crates/winit).

The library translates winit events to egui, handled copy/paste, updates the cursor, open links clicked in egui, etc.

## 公文助手补丁

`gongwen_application_paste` 启用时，窗口后端保留粘贴按键，由公文助手选择应用内
剪贴板或在后台读取系统剪贴板，同时禁用 Linux PRIMARY 主选区的自动读写。
系统剪贴板输出仍由原生后端处理，应用在 egui 的最终平台输出上做确认闸门。
