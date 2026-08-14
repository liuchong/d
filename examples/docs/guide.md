# 快速上手指南

## 安装

```bash
cargo install d
```

## 启动

```bash
# 服务当前目录
d

# 指定目录和端口
d -r ./public -p 3000
```

## 常用技巧

1. **断点续传**：`curl -C - -O http://localhost:8080/bigfile.zip`
2. **下载部分文件**：`curl -r 0-1023 http://localhost:8080/data.bin`
3. **只看头部**：`curl -I http://localhost:8080/data.bin`

## 目录页查询参数

- `?sort=name|size|time|type` — 排序
- `?hidden=true|false` — 显示/隐藏隐藏文件（需服务端 `--hidden`）

## 文件查看参数

- `?view=raw` — 原始内容
- `?view=download` — 作为附件下载
