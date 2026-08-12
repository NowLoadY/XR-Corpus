# Corpus Markdown schema v1

Corpus 是纯 Markdown 文档，不嵌入 JSON、YAML 或其它隐藏元数据块。标题和二级标题表达结构，
术语按行读取，因此人、Agent 和程序看到的是同一份内容。

## 固定语言顺序

所有 `Triggers` 与 `Terms` 行必须严格使用以下顺序：

```text
zh,en,fr,pt,es,ja,ru,ko,th,it,de,vi,id,pl,cs,nl
```

对应语言依次为：中文、英语、法语、葡萄牙语、西班牙语、日语、俄语、韩语、泰语、意大利语、
德语、越南语、印度尼西亚语、波兰语、捷克语、荷兰语。

- 一行只表达一个概念，必须正好有 16 个字段。
- 字段之间只使用 ASCII 英文逗号 `,`。
- 某语言没有可靠对应术语时字段留空，但逗号必须保留，禁止移动后续字段。
- 术语本身不得包含英文逗号或换行；需要表达列表时拆成多条概念。
- 同一概念的品牌名或缩写在多种语言中不变时，可以在对应列重复填写。

## 文件结构

```markdown
# Corpus 标题

> 文件说明，并明确本文件遵循固定语言顺序。

## Metadata

schema: xrtranslate-corpus/v1
priority: 20
activation: on-evidence

## Language Order

zh,en,fr,pt,es,ja,ru,ko,th,it,de,vi,id,pl,cs,nl

## Triggers

虚拟现实,virtual reality,réalité virtuelle,realidade virtual,realidad virtual,仮想現実,виртуальная реальность,가상 현실,ความเป็นจริงเสมือน,realtà virtuale,virtuelle Realität,thực tế ảo,realitas virtual,wirtualna rzeczywistość,virtuální realita,virtuele realiteit

## Trigger Aliases

VR,VR,VR,VR,VR,VR,VR,VR,VR,VR,VR,VR,VR,VR,VR,VR

## Activation Context

美食,food,cuisine,comida,comida,料理,еда,음식,อาหาร,cibo,Essen,ẩm thực,makanan,jedzenie,jídlo,eten

## Terms

头戴式显示器,head-mounted display,,,,ヘッドマウントディスプレイ,,헤드 마운트 디스플레이,,,,,,,,

## Scope

供维护者阅读的范围和来源说明；该节不会注入 ASR。
```

## 读取规范

- 第一个一级标题 `# ` 是 corpus 标题。
- `Metadata`、`Language Order`、`Triggers`、`Terms` 四个二级标题必须存在。
- `schema` 当前必须是 `xrtranslate-corpus/v1`；`priority` 必须是有符号整数。
- `activation` 可选，默认 `on-evidence`。静态文件只应在极小、稳定、全局有价值的基础入口词上使用 `always`；运行时短期事实可使用 `always` 或 `runtime-only`。不要用运行时集成作为基础名称能否被识别的前置条件。
- `Language Order` 必须与规范逐字符一致，不能交换或省略语言。
- `Triggers` 用于根据最近成功转写和未来实时 hints 激活 corpus；规范 Trigger 可以表达标题等稳定双语映射。Trigger 是选择证据，不是偏置词表。
- 可选的 `Trigger Aliases` 保存近义词、缩写、口语和拼写变体。它们参与激活与来源高亮，但永不进入 ASR 或翻译 prompt。
- 可选的 `Activation Context` 提供第二组必要证据。存在该节时，至少一个 Trigger（含 Alias）和至少一个 Activation Context 必须同时出现在近期证据中，corpus 才能激活。
- 国家、人物等跨主题实体应使用 `Activation Context` 限定主题。例如日本美食要求“日本/日本的”与“美食/料理”共同出现；单独提到日本不能激活美食或人物 corpus。
- `Terms` 是可按当前翻译任务语言选择并注入 ASR/翻译 prompt 的偏置词与术语。
- 空行和以 `>` 开头的说明行不作为数据；其它未定义二级节只供维护者阅读。
- `id`、`domain`、`subdomain` 由规范目录路径推导，不在文件内重复维护。`corpora/` 下可使用嵌套目录组织海量概念：
  `<domain>.<subdomain>.<relative-corpus-path-with-dots>`，例如
  `entertainment.anime-and-manga.works.sora-no-otoshimono.characters`。

## 内容选择规范

语料预算优先解决小模型容易听错、拼错或尚未学到的概念，而不是充当普通词典：

- `Triggers` 可以包含少量基础领域词，仅用于激活 corpus；不要因此把这些词重复放入 `Terms`。
- 基础入口词若本身就必须默认识别，应放入小型 `activation: always` 默认语料的 `Terms`，而不是依赖某个集成运行后才触发。
- 同一领域应补充克制的近义词、缩写、全称、常见口语和拼写变体作为独立 Trigger 行。
- 避免单个字、过短缩写和跨领域高频词；Trigger 必须在正常对话中足以成为该领域的有效证据。
- Trigger 永远不是翻译约束。只有与其明确重合的 `Terms` 才能进入 ASR 热词和翻译 prompt。
- `Terms` 优先收录产品名、人物名、作品名、模型与框架、缩写、社群用语、固定本地化译名及近期热词。
- 不收录模型通常能稳定处理的宽泛基础词；不能明显提升 ASR 或翻译准确率的行应删除。
- 一个 corpus 只覆盖一个足够窄的主题。快速变化的模型、版本或作品名册与稳定概念分文件维护。
- 海量作品、角色、人物和地点使用概念网式分层：领域/子领域下按 `works/<work-id>/`、`people/<person-id>/`、`places/<place-id>/` 等目录拆分。作品级角色 corpus 应允许作品名和核心角色名都作为召回入口，使“提到作品带出角色”和“提到角色联想到作品”同时成立。
- 高价值且新近的词放在 `Terms` 前部，使上下文预算不足时仍优先入选；不要依赖无限 prompt。
- 热词和社区表达必须能追溯到官方资料或可信的一手社区资料，并在文件末尾的 `Source` 或 `Sources` 节记录。
- 版本号只在本身已成为常用专名且基础模型可能未知时收录；新版本替代旧版本后及时清理。

加载阶段严格检查文件大小、文件数量、目录层级、语言列数、重复记录和字段长度。任何 corpus
不合法时后端拒绝启动并报告具体文件与行号，避免静默错列后向 ASR 注入错误语言。

代码生成或 Agent 写入新 corpus 时应先构造 `CorpusDefinition` / `CorpusTerm`，再调用
`CorpusDefinition::to_markdown()` 生成规范正文；人工维护必须遵循同一模板。读取端与规范写入端
共享同一个语言顺序常量，不能各自维护列顺序。

## 动态 Source 规范

API 或其它后台程序不写入静态 Markdown。它们构造同一个 `CorpusDefinition` / `CorpusTerm` 模型，
每个 `CorpusTerm.ordered_values` 同样严格对应上述 16 个位置，并通过原子 snapshot 与可选 TTL 发布。
因此静态与动态语料共用触发、排序、语言选择和 prompt 预算逻辑。
