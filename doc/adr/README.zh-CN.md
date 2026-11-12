# ADR 索引

> Language: [English](README.md) | **简体中文**

Loom 的架构决策记录。ADR 记录的是“为什么这么选”和“当时否掉了哪些路”——半年后腐烂成“我们当初为什么这么做”的正是这部分。只给难以回退、或跨模块边界的决定写 ADR；日常选择留在设计文档里。

## 格式

文件名 `ADR-NNNN-kebab-title.md`，编号取下表中的下一个。模板：

````markdown
# ADR-0007: <标题>

Status: Proposed | Accepted | Deprecated | Superseded by ADR-00xx

## Context
什么迫使现在做这个决定：约束、问题形态、现状为什么疼。

## Decision
选了什么。通常三行就够。

## Consequences
什么变容易了、什么变难了、这个决定派生了哪些后续工作。

## Alternatives rejected
其他方案为什么出局。这一节才是 ADR 的价值所在。

## Model assumptions
这个决定依赖模型的什么能力（例如“中转的 chat wire 接受 reasoning_effort=high”、
“官方 deepseek 的 usage 数字可信”），以及什么变化触发复审（模型升级、换供应商、换 wire）。
````

## 规则

- 已接受的 ADR 永不修改。推翻它就新写一篇，把旧文标为 `Superseded by ADR-NNNN`；原文留作历史。
- **Model assumptions** 一节必填。建立在模型行为上的决定会随模型换代失效，这一节就是复审触发清单。
- 每篇 ADR 都登记进下表，并从 `doc/README.zh-CN.md` 链接进来。
- 语言遵循 `AGENTS.md` §8：英文为主，配 `*.zh-CN.md` 姊妹文件。

## 索引

| ADR | 标题 | 状态 |
|---|---|---|
| — | 暂无 | — |
