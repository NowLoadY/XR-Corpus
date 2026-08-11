# VRChat Instance 与社交功能

> 本文件遵循 `corpora/v1/SCHEMA.md`。普通的“房间”和“世界”只用于触发；Terms 留给 VRChat 特有实例类型和社交功能。

## Metadata

schema: xrtranslate-corpus/v1
priority: 45

## Language Order

zh,en,fr,pt,es,ja,ru,ko,th,it,de,vi,id,pl,cs,nl

## Triggers

VRChat,VRChat,VRChat,VRChat,VRChat,VRChat,VRChat,VRChat,VRChat,VRChat,VRChat,VRChat,VRChat,VRChat,VRChat,VRChat
实例,instance,instance,instância,instancia,インスタンス,инстанс,인스턴스,อินสแตนซ์,istanza,Instanz,phiên bản,instance,instancja,instance,instantie
群组实例,group instance,,,,グループインスタンス,,그룹 인스턴스,,,,,,,,

## Trigger Aliases

房间,room,,,,ルーム,,방,,,,,,,,
世界,world,,,,ワールド,,월드,,,,,,,,
大厅,lobby,,,,ロビー,,로비,,,,,,,,
邀请,invite,,,,招待,,초대,,,,,,,,
请求邀请,request invite,,,,招待リクエスト,,초대 요청,,,,,,,,
加入我,join me,,,,参加して,,나에게 참가해,,,,,,,,
公开房,public instance,,,,パブリックインスタンス,,공개 인스턴스,,,,,,,,
好友房,friends instance,,,,フレンドインスタンス,,친구 인스턴스,,,,,,,,

## Terms

Public 实例,Public instance,,,,Publicインスタンス,,Public 인스턴스,,,,,,,,
Group 实例,Group instance,,,,Groupインスタンス,,Group 인스턴스,,,,,,,,
Group+ 实例,Group+ instance,,,,Group+インスタンス,,Group+ 인스턴스,,,,,,,,
Friends+ 实例,Friends+ instance,,,,Friends+インスタンス,,Friends+ 인스턴스,,,,,,,,
Invite+ 实例,Invite+ instance,,,,Invite+インスタンス,,Invite+ 인스턴스,,,,,,,,
Invite 实例,Invite instance,,,,Inviteインスタンス,,Invite 인스턴스,,,,,,,,
群组公开实例,Group Public instance,,,,Group Publicインスタンス,,Group Public 인스턴스,,,,,,,,
邀请请求,request invite,,,,招待リクエスト,,초대 요청,,,,,,,,
信任等级,Trust Rank,,,,トラストランク,,신뢰 등급,,,,,,,,
Quick Menu,Quick Menu,Quick Menu,Quick Menu,Quick Menu,Quick Menu,Quick Menu,Quick Menu,Quick Menu,Quick Menu,Quick Menu,Quick Menu,Quick Menu,Quick Menu,Quick Menu,Quick Menu
Main Menu,Main Menu,Main Menu,Main Menu,Main Menu,Main Menu,Main Menu,Main Menu,Main Menu,Main Menu,Main Menu,Main Menu,Main Menu,Main Menu,Main Menu,Main Menu
Earmuffs,Earmuffs,Earmuffs,Earmuffs,Earmuffs,Earmuffs,Earmuffs,Earmuffs,Earmuffs,Earmuffs,Earmuffs,Earmuffs,Earmuffs,Earmuffs,Earmuffs,Earmuffs

## Dynamic Boundary

当前世界、实例和玩家等短期事实只由运行时动态 Source 提供，不写入静态文件。
