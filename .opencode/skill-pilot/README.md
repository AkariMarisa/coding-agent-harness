# Skill 试点说明

这是 Loom 仓库内的项目级 skill 试点，不安装到 `~/.config/opencode/skills`。

位置：`.opencode/skills/`。OpenCode 在当前项目内自动发现；队友克隆仓库后即可使用，不需要额外安装。

## 1. 当前接入的 skill

| ID | 作用 | 自动广告 | 正文成本（约） | 参考文件按需加载 |
|---|---|---|---:|---|
| `docs-writing` | 文档写作、结构、Markdown 格式、去 AI 味 | 是 | 约 0.6K tokens | `ai-tells.md` 约 1.0K；`markdown.md` 约 0.5K |
| `code-quality-review` | 架构边界、依赖方向、循环依赖、code smell、重构评估 | 是 | 约 1.1K tokens | `architecture.md` 约 0.7K；`smells.md` 约 0.7K |
| `format-code` | Rust、YAML、SQL、JSON、Markdown、Shell、TOML 格式化 | 是 | 约 0.7K tokens | `formatters.md` 约 0.6K |

正文只在模型调用 skill 时加载，参考文件只在对应模式需要时读取。不会每次对话都发送全文。

## 2. 触发范围

触发范围过宽是 skill 最大的隐性成本：描述被模型选中，正文就会进入上下文；如果每次回答都触发，省下的沟通成本会被 token 吃掉。

当前三个 skill 都写了明确的 `Trigger gate`，并在 `description` 里写出“不要用于什么”。

| ID | 应该触发 | 不应该触发 |
|---|---|---|
| `docs-writing` | 新建或大幅改写 README、设计文档、ADR、报告、发布说明；用户明确要求润色、去 AI 味、格式化文档 | 普通对话回答、代码注释、提交信息、短状态更新、纯事实问答 |
| `code-quality-review` | 用户明确要求 review/audit；检查模块边界、依赖方向、循环依赖；找 code smell；评估重构方案 | 普通功能实现、每次代码修改后的自检、格式化/lint、单行 bug 修复 |
| `format-code` | 用户明确要求格式化/检查格式；目标文件类型已配置 formatter，且任务就是格式化这些文件 | 普通代码编辑、提交、lint 错误、测试失败、项目没有 formatter 配置 |

一个需要特别说明的决定：**“回答风格”没有单独做成 skill**。如果每次回答都自动加载写作规范，正文会反复进入上下文，成本高于收益。当前把“结论先行、具体、保留语气”三条放进 `docs-writing` 的 baseline，只在文档任务中生效；普通回答沿用 `AGENTS.md` 的默认语言与风格约定。若以后确实需要全局回答风格，应加到 `AGENTS.md` 这类常驻指令，而不是做成自动 skill。

## 3. 固定成本

OpenCode 在每个模型步骤广告已启用 skill 的 ID、名称和描述，不发送正文。三个描述合计约 150 tokens/step。

- 50 步的会话：约 7.5K tokens 的固定广告成本。
- 单次误触发：额外加载约 0.6K 到 2K tokens。
- 用 `metadata.opencode/autoinvoke: false` 可以让某个 skill 不进入广告列表，但用户必须显式用 `@skill-id` 调用；当前保留自动广告，因为三个描述都足够窄。

如果后续 skill 数量增加，优先压缩描述，而不是删除触发边界。描述应回答“做什么 + 什么时候用 + 什么时候不用”，不超过两三句。

## 4. 来源与许可

这些 skill 是项目内改编版，不是逐字复制。来源均为 MIT 许可，版权与许可全文见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。

| 来源 | 版本 | 用途 |
|---|---|---|
| [`petergyang/no-ai-slop`](https://github.com/petergyang/no-ai-slop) | `000650b` | 去 AI 味规则、检测模式 |
| [`mattpocock/skills`](https://github.com/mattpocock/skills) | `4ae2317` | Fowler code smell baseline、架构深化思路 |
| [`addyosmani/agent-skills`](https://github.com/addyosmani/agent-skills) | `301b02b` | 文档/ADR 约定、代码简化原则 |

Markdown 格式规则和 formatter 命令表是按通用工具文档整理的，不依赖某个特定 skill。

## 5. 验证

试点阶段用固定提示检查三件事：

1. 普通回答不会触发 `docs-writing`。
2. “帮我 review 这个 diff 的架构问题”只触发 `code-quality-review`。
3. “格式化这些 YAML/SQL”只触发 `format-code`，且不会顺手改语义。

后续如果 Loom 实现 skill 扫描，应把“描述长度、触发/不触发样例、实际加载次数、误触发成本”纳入 skill 验证，而不是只看它写得好不好。
