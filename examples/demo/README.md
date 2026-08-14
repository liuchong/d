# d 演示目录

这个目录用于演示 **d** 的目录列表与文件预览能力。

## 试试这些操作

- 点击 `hello.rs` —— 代码文件会显示 **Preview / Raw / Download** 三个标签页，带语法高亮
- 点击本文件（`README.md`）—— Markdown 会被渲染成排版后的页面
- 点击 `notes.org` —— org-mode 基础渲染
- 点击 `logo.svg` —— 图片直接预览
- 点击右上角排序按钮 —— 按 名称 / 大小 / 时间 / 类型 排序

## 表格渲染示例

| 文件 | 类型 | 演示点 |
|------|------|--------|
| `hello.rs` | Code | 语法高亮 |
| `data.json` | Text | JSON 预览 |
| `report.csv` | Text | 表格数据 |
| `logo.svg` | Image | 图片预览 |

## 代码块渲染示例

```rust
fn main() {
    println!("Hello from d!");
}
```
