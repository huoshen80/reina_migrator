# Reina Migrator - 为ReinaManager提供的数据迁移工具

这是一个用于将 Whitecloud 或 Playnite 游戏数据迁移到 ReinaManager 的工具。

## 适用于
- ReinaManager v0.29.1 及以上版本
- Whitecloud v0.4.0 数据库结构
- 由 Reina Exporter 导出的 Playnite 10 游戏库

## 功能特性

- 使用 SeaORM 进行数据库操作
- 支持 SQLite 数据库
- 自动迁移游戏数据、会话记录和统计信息
- 支持 ReinaManager 安装版与便携版
- 支持迁移 Playnite 元数据、启动配置、累计统计和封面

## 使用

准备工作：

- 至少启动过一次 ReinaManager，确保目标数据库已经存在。
- 从 Whitecloud 迁移时，找到 `db.3.sqlite`（一般位于 `whitecloud安装路径\resources\data\db.3.sqlite`），并将其放在迁移器同一目录。
- 从 Playnite 迁移时，安装独立项目 `Reina-Playnite-Exporter` 生成的 Reina Exporter 插件，在 Playnite 的扩展菜单选择 `Export library for ReinaManager`，保存 JSON 文件。在完成迁移前不要移动或删除 Playnite 的封面文件。

运行：

1. 双击可执行文件运行。

2. 选择迁移来源：Whitecloud 或 Playnite。选择 Playnite 时，在文件窗口中选择 Reina Exporter 生成的 JSON。

3. 选择 ReinaManager 版本：
   - 安装版：输入 `1` 或直接按 Enter。
   - 便携版：输入 `2`，然后在弹出的目录窗口中选择包含 ReinaManager 的根目录。

4. 如果 ReinaManager 正在运行，请先保存数据并手动退出。按 Enter 重新检测；输入 `0` 可取消迁移。

5. 程序在迁移前会备份数据库到 ReinaManager 设置的数据库备份目录；未设置或目录无效时，保存到目标数据库同目录下的 `backups/` 文件夹。文件名格式类似：

   `reina_manager_20250820_154719_178.db`

6. 迁移完成后，程序会提示按 Enter 退出。

注意：迁移器不会关闭 ReinaManager 进程。请在迁移前手动退出 ReinaManager，以保证数据库完整性。
## 数据映射关系

### Playnite -> ReinaManager

- `games.id_type` 固定为 `Playnite`，不写入 `game_sources`。
- 名称、排序名称、简介、标签、开发商、用户评分、用户评价和成人标记写入 `custom_data`。
- 发行日期、添加时间、修改时间和五种游玩状态写入 `games` 对应字段。
- Steam 游戏写入 Steam AppID；本地游戏写入展开后的目录和启动文件名。无可表示启动项的游戏仍会导入资料。
- 累计游玩秒数、次数和最近游玩时间写入 `game_statistics`，不会伪造 `game_sessions`。因为 Playnite 不提供逐次会话，ReinaManager 将来若主动重建统计，导入的累计基线可能被清除。
- 两种来源都按 Steam AppID 或“游戏目录＋启动文件”判重；无可用标识的游戏仍导入，再次迁移可能重复。
- 匹配到已有游戏时，只在目标无会话且游玩时长、次数为空或零时补充统计；已有非零统计不会覆盖或累加。Whitecloud 还可用 `saveDir` 补充空的存档路径。
- 一个标识对应多个 ReinaManager 游戏时会跳过并报告歧义。Whitecloud 来源中相同启动项的会话先合并、去重后写入；若这些来源记录的存档路径相互冲突，则不补该路径。
- 本地或 HTTP(S) 封面会复制到 ReinaManager 的封面目录；单个封面失败不会中止游戏迁移。
- ReinaManager v0.29.1 当前尚未把 `Playnite` 纳入“自定义游戏”筛选，但游戏仍会正常出现在全部游戏和本地游戏中。

Playnite 状态映射：

| Playnite | ReinaManager | `clear` |
|---|---|---:|
| Not Played、Plan to Play | 想玩 | 1 |
| Played、Beaten、Completed | 玩过 | 2 |
| Playing | 在玩 | 3 |
| On Hold | 搁置 | 4 |
| Abandoned | 抛弃 | 5 |
| 其他自定义状态 | 想玩 | 1 |

### 旧数据库 -> 新数据库

#### games 表映射：
- `gameDir` -> `localpath`（游戏目录）
- `exePath` -> `executable`（启动文件名）
- `saveDir` -> `savepath`
- `name` -> `custom_data` JSON 中的 `name` 字段
- `uuid` -> 用于关联其他表的数据
- 固定值：
  - `id_type` = "Whitecloud"
  - `clear` = 1
- 未映射字段：
  - `game_sources` 外部数据源记录不写入
  - `autosave`、`maxbackups`、`le_launch`、`magpie`、`created_at`、`updated_at` 等字段由 Reina 数据库默认值处理

#### 时间处理：
- 迁移时不再单独迁移游戏时间字段，所有时间相关内容通过会话和统计表处理

#### 会话记录：
- 从 `history` 表迁移到 `game_sessions` 表
- 计算游戏时长和统计信息

## 数据映射


### 游戏表 (games)

| 旧字段 | 新字段 | 说明 |
|--------|--------|------|
| gameDir | localpath | 游戏目录 |
| exePath | executable | 启动文件名 |
| saveDir | savepath | 存档路径 |
| uuid | - | 用于关联其他表 |
| - | id_type | 固定为 "Whitecloud" |
| - | clear | 固定为 1 |
| - | custom_data | JSON 列，包含 name |

未映射的 Reina 字段不由迁移器声明或写入；数据库会使用 `NULL` 或表结构中的默认值。

### 时间处理

- 迁移时不再单独迁移游戏时间字段，所有时间相关内容通过会话和统计表处理

### 游戏会话 (game_sessions)

- 从 `history` 表迁移游戏会话数据
- 计算会话持续时间
- 生成统计信息
