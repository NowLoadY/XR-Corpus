# XRTranslate 专业语料库

该目录是运行时可加载的内容资产，不是源代码资源。目录版本与文档 schema 独立演进；当前版本为
`v1`。

稳定层级只有三层概念：

1. `domain`：长期稳定的大领域，例如 `virtual-worlds`；
2. `subdomain`：领域内稳定的专业方向，例如 `vrchat`；
3. `corpus`：可独立触发、注入和设定优先级的小型语料单元。

不要用额外的无语义目录深度表达“更大”或“更小”。需要更细内容时，应新增 corpus，或在确有
长期分类价值时新增 subdomain。运行时只加载以下固定位置的 Markdown：

```text
v1/domains/<domain-id>/subdomains/<subdomain-id>/corpora/<corpus-id>.md
```

`domain.md` 与 `subdomain.md` 是分类说明，不会作为 prompt 内容加载。Corpus 格式见
[`v1/SCHEMA.md`](v1/SCHEMA.md)。

相对路径始终以 `config.json` 所在目录为根。发布构建会把整个 `corpora/` 复制到 release 根，
并把 release 配置明确改写为 `corpora/v1`；后端 exe 是否位于 `bin/` 不影响解析。

## 静态与动态数据的边界

Markdown 只保存稳定、可审阅、可随版本发布的专业术语，不嵌入 JSON/YAML 元数据。每个概念
独占一行，并按 `zh,en,fr,pt,es,ja,ru,ko,th,it,de,vi,id,pl,cs,nl` 固定列顺序存储。房间名、玩家名称、人数等短期状态不写入
静态文件。后端的 corpus catalog 同时聚合 Markdown source 与 runtime dynamic source；VRCX
只读适配器及未来 API/后台适配器都按 provider 发布完整快照，并可附带 TTL。一次更新是原子替换，不会让 ASR 读到
一半新、一半旧的房间状态。两类 source 输出相同的 `xrtranslate-corpus/v1` 数据结构，因此触发、
排序和 prompt 预算逻辑可以完全复用。`Always` 动态语料的术语同时作为静态语料触发证据，因此
世界名或玩家显示名中出现游戏、作品或角色关键词时，会自动激活对应的专业 corpus。
