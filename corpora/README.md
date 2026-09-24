# XR Corpus 默认词汇图

`default.sqlite` 是新安装的种子数据库。首次运行时，XR Corpus 将它复制到安装目录的
`runtime/xr-corpus.sqlite`；此后只读写 `runtime` 中的用户数据库。软件更新会保留该文件。

数据库使用 SQLite `user_version=1` 和 DELETE journal。退出 XRTranslate 后，可以直接复制
`runtime/xr-corpus.sqlite`，向另一位用户分享自己的领域、词汇和触发关系。

一个词汇节点保存 16 种语言中已知的表达，顺序为：

```text
zh,en,fr,pt,es,ja,ru,ko,th,it,de,vi,id,pl,cs,nl
```

节点必须归属一个领域。关闭领域或节点会停止其词汇与触发关系；关闭一条关系只停止该关系。
关系可指向尚不存在的节点，在该节点出现并启用后自动生效。`trigger` 关系提供激活证据，
`context` 关系要求再出现一个语境词汇。`on-evidence` 节点没有有效触发入边时保持闲置，
`always` 节点始终可作为候选词汇。运行时提供者的短期数据仅保存在内存中。
