# Reina Migrator - 为ReinaManager提供的数据迁移工具

这是一个用于将 Whitecloud 数据迁移到ReinaManager的工具。

## 适用于
- ReinaManager v0.7.0 及以上版本

## 功能特性

- 使用 SeaORM 进行数据库操作
- 支持 SQLite 数据库
- 自动迁移游戏数据、会话记录和统计信息

## 使用

准备工作：
- 找到 `db.3.sqlite` 文件，它一般位于 `whitecloud安装路径\resources\data\db.3.sqlite`
- 将 `db.3.sqlite` 放在程序同一目录

运行：

1. 双击可执行文件运行：

2. 程序会在迁移前自动备份目标数据库到同目录下的 `backups/` 文件夹，命名格式类似：

   `reina_manager_2025-08-20T07-47-19-178Z.db`

3. 迁移完成后，程序会提示按任意键退出。

注意：请在迁移前确保已保存 ReinaManager 中的所有未保存数据，迁移期间将尝试关闭 ReinaManager 进程以保证数据库完整性。

## 数据映射关系

### 旧数据库 -> 新数据库


#### games 表映射：
- `gameDir` + `exePath` -> `localpath` (组合路径)
- `saveDir` -> `savepath`
- `uuid` -> 用于关联其他表的数据
- 固定值：
  - `id_type` = "Whitecloud"
  - `clear` = 0
  - `autosave` = 0
  - 迁移时自动生成 `created_at`、`updated_at`

#### other_data 表映射：
- `name` -> `name`
- 固定值：
  - `image` = "/images/default.png"
  - 其余字段如 `summary`、`tags`、`developer` 暂为 None


#### 时间处理：
- 迁移时不再单独迁移游戏时间字段，所有时间相关内容通过会话和统计表处理

#### 会话记录：
- 从 `history` 表迁移到 `game_sessions` 表
- 计算游戏时长和统计信息

## 数据映射


### 游戏表 (games)

| 旧字段 | 新字段 | 说明 |
|--------|--------|------|
| gameDir + exePath | localpath | 游戏本地路径 |
| saveDir | savepath | 存档路径 |
| uuid | - | 用于关联其他表 |
| - | id_type | 固定为 "Whitecloud" |
| - | clear | 固定为 0 |
| - | autosave | 固定为 0 |
| - | created_at/updated_at | 迁移时自动生成 |

### 其他数据表 (other_data)

| 旧字段 | 新字段 | 说明 |
|--------|--------|------|
| name | name | 游戏名称 |
| - | image | 固定为 "/images/default.png" |
| - | summary | 暂无，保留字段 |
| - | tags | 暂无，保留字段 |
| - | developer | 暂无，保留字段 |

### 时间处理

- 迁移时不再单独迁移游戏时间字段，所有时间相关内容通过会话和统计表处理

### 游戏会话 (game_sessions)

- 从 `history` 表迁移游戏会话数据
- 计算会话持续时间
- 生成统计信息
