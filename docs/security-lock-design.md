# 应用锁与数据加密设计（双密码 + SQLCipher）

2026-11-06：第三期全生命周期细化见 `encrypted-resource-lifecycle.md`，涵盖导入、
历史资源、稿件包、全量备份恢复、中间文件、Pi 与明文输出确认。该文取代本篇
“可重建 Mermaid 缓存不加密”的旧取舍；加密模式的明文输出需每次确认风险。
实际实现范围以新文末节为准，不表示图片/Pi 加密已经完成。
同日第二轮加固（输出确认补漏、密封名单统一、失败态降级防护、停用标记认证、
双槽备份止血、图片/词表/技能槽位隔离与权限收紧）见该文末节；本轮不改变磁盘资源
编码，B–E 仍未实施。

2026-11-04 与用户确认的技术路线：数据库加密走 **SQLCipher**（rusqlite 现成 feature，
页级透明加密，检索 / SQL / 索引零改动）；加密覆盖范围为**全部敏感数据**（稿件库、
config.json 与密钥文件、内嵌图片目录、Pi 运行目录）。

## 一、目标与威胁模型

### 要防的

- 拷走整个配置目录（U 盘复制、网盘同步、压缩外发）后离线翻看 `manuscripts.db`、
  `config.json`、内嵌图。
- 拆走硬盘 / 偷走关机状态的机器后读取数据。
- 被迫交出密码的场景下，交出的假密码打开一个**行为完全可信的空系统**，
  不暴露还有另一套数据存在。

### 不防的（写进使用帮助，避免过度承诺）

- 解锁状态下机器已中恶意软件（读进程内存、截屏、键盘记录）——任何同类软件都防不住。
- 输密码时被旁观。系统整体沦陷不在本方案范围。

### 威胁模型的直接推论

- 内存中 SQLite 页是明文（用户明确确认：内存不加密）。SQLCipher 恰好就是这个模型：
  页级透明加密，读进内存才解密。
- 每次启动都要输密码；密码不落盘（除可选的「本机记住」机制，默认关，见 4.4）。

## 二、总体结构：密码 → 密钥 → 一切

```
密码（真 / 假两套）
   │  Argon2id(密码, 各自盐)                ← 新增依赖 argon2（纯 Rust）
   ▼
密钥 A（真） / 密钥 B（假）
   │
   ├─→ 验证器（两个并排的 AEAD sealed blob，结构完全一致，解不开就是密码错）
   ├─→ SQLCipher PRAGMA key（manuscripts.db，库与诱饵库同构、文件名中性不可区分）
   └─→ config.json / api-secrets.json / images/ / pi/ 的 AEAD 加解密
```

**根基不变量：磁盘上不存在任何能区分「这是一套还是两套密码」「哪个是真密码」的结构。**

- 两个验证器并排存放，尺寸、格式、字段完全一致；Argon2id 参数固定，验证器尺寸恒定。
- 真库与诱饵库文件名中性同构（统一命名规则，按验证器序号绑定，绝不出现
  real/decoy 语义），WAL 文件同样加密（SQLCipher 对 WAL 页同样加密），
  文件级看过去无法分辨哪个是哪个、里面有没有数据。
- 假密码解锁后进入**真·空库**：功能完整、可新建稿件、数据真实保存进诱饵库。
  界面行为与「一台还没录入数据的机器」一致，无可指摘。

## 三、需求一：锁屏与双密码（第一期，纯应用层）

### 3.1 锁屏

- 在 egui 主窗口内做锁屏界面（不另起进程）；`GongwenApp::new` 里
  `storage::load()`（src/app.rs:459）与四处 `ManuscriptStore::open`
  （src/app.rs:531/538/552/570 等）目前都发生在构造期，**全部延后到解锁之后**。
- 解锁前的降级行为：`peek_theme`（src/storage.rs:50）读不到加密配置就回落默认主题，
  锁屏反正盖着界面，可接受。
- 单次会话内已解锁；支持手动「锁定」（清密钥句柄、回到锁屏、数据句柄全部重建）。
- 单实例转交（`.gwx` 双击交给已开窗口）不做二次解锁——进程已解锁。

### 3.2 双密码与验证器

- 新增 `src/vault/` 模块：`derive_key(password, salt) -> [u8; 32]`（Argon2id，
  m=64MiB、t=3、p=4，参数写死并记入文档）、`seal` / `open`（AES-256-GCM，
  纯 Rust 依赖 `aes-gcm` 或 `chacha20poly1305`，二选一，推荐 AES-256-GCM）。
- 解锁流程：输入密码 → 用固定盐池逐个试解两个验证器（Argon2id 只跑一次，
  两个验证器用同一派生密钥试解即可，顺序固定）→ 命中哪个开哪个库。
  **错误提示统一为「密码不正确」**，不区分「没有这个密码」。
- 两个密码在设置页分别可改（改密 = 重派生密钥 + 全量重加密对应库 / 配置文件，
  用 SQLCipher 的 `PRAGMA rekey` 与 AEAD 重封，原子替换）。
- 首次启用向导：设真密码 → 可选设假密码（也可后补）→ 明文数据迁移（见第五期说明）。
- **忘记密码 = 数据不可恢复，无后门。** 设置页与首次启用向导都必须明确警告。

### 3.3 诱饵系统

- 假密码命中 → 打开诱饵库（独立 SQLCipher 库，同一 schema，空数据），
  `config.json` 等配置同样有两份（真 / 假各自独立演进）。
- 假系统里产生的数据真实保存，绝不静默合并回真库；两套数据互不越界。
- 「整套停用」：设置页可把功能关掉回到现状（无锁屏、明文库），
  停用不删数据；再启用时按现有解锁方式接管。

## 四、需求二：SQLCipher 与加密范围（第二期起）

### 4.1 rusqlite 接入

- `Cargo.toml`：`rusqlite` feature 从 `["backup", "bundled"]` 改为
  `["backup", "bundled-sqlcipher-vendored-openssl"]`。三平台纯源码构建，
  不引入系统依赖；代价是 CI 编译时间增加（vendored OpenSSL 每次约几分钟，
  缓存键会变化），Windows / Linux ARM64 / macOS 三条 CI 都要真机验证一把。
- 打开流程：每个 `Connection::open` 之后立即 `PRAGMA key = "x'<hex>'"`。
  **注意**：当前有多处瞬态开库（后台任务 src/app/jobs.rs、AI 服务
  src/ai_services/*.rs、同步导入等共十余处 `ManuscriptStore::open`），
  不能每处传密码。做法：解锁后把密钥放进进程级句柄（`Arc<VaultKey>` 或
  线程局部），所有开库路径统一从句柄取 key；未解锁时这些路径本就不该被触发。
- 验证 key 有效性：`PRAGMA cipher_integrity_check` 或试跑一句已知 schema 查询；
  假密码派生的 key 解真库必然失败——这本来就是需求一想要的行为，
  但代码里要把「密文损坏」和「密码不对」的错误文案统一掉，避免泄露库的存在性。

### 4.2 检索为什么不受影响（对用户的承诺，也写进帮助）

SQLCipher 是**页级透明加密**：SQLite 按页读写磁盘，页落盘前加密、读入内存后解密。
SQL、B-tree 索引、LIKE、FTS5 全文检索、ORDER BY 全部跑在内存明文页上，
**一行查询代码都不用改**。性能开销：每页一次 AES 加解密，现代 CPU 上可忽略
（565 MB 库的日常读写无感）。

### 4.3 加密范围清单

| 路径 | 处置 |
|---|---|
| `manuscripts.db`（+WAL） | SQLCipher，真 / 假两套库 |
| `config.json` | 整份 AEAD 加密（含词库：单位 / 人名 / 电话），原子写 |
| `api-secrets.json`、`api-envs.json`、`api-tests.json`、`.zip-password` | AEAD 加密（`.zip-password` 现状明文存 ZIP 密码，必须纳入） |
| `apis/`（OpenAPI 定义，一般不含密） | 建议一并加密，省事且口径统一 |
| `images/`（正文内嵌图） | 逐文件 AEAD（头 + 密文），扩展名不变、文件名哈希化或保持原样需斟酌（见 6.2） |
| `pi/`（运行目录，含 sources 材料） | 整目录纳入；目录内小文件多，建议整目录打成一个加密容器文件或逐文件 AEAD |
| `help/`（随包帮助，不敏感） | 不加密 |
| `mermaid-cache/`（可重建缓存） | 不加密，顺手挪去 `cache_dir()`（src/storage.rs:35），别留在配置目录 |
| `logs/`、`metrics.json` | 低敏感；先清理内容（确保不抄正文片段），不加密 |
| 导出成品（PDF / DOCX / 备份包 / `.gwx`） | 不加密：用户主动存到自选输出目录，须保持可用；`.gwx` 本就另有 ZIP 密码 |

### 4.4 可选「本机记住」（默认关）

默认每次启动输密码。可选「本机记住」：把密钥用 Windows DPAPI /
macOS Keychain / Linux libsecret 保护后存本机——**脱离当前 Windows 用户账户就解不开**，
拷走数据目录无效。三平台各有 API，工作量不小，列为第三期之后的可选项，第一期不做。

## 五、迁移与备份链路

- **明文 → 加密迁移**（首次启用向导的最后一步）：SQLCipher 标准流程
  `ATTACH ... KEY ...; SELECT sqlcipher_export(...);` 把明文库整体搬进加密库，
  原子替换，旧明文文件**原地覆写改写后删除**（不只 delete，防恢复）。
  `config.json` 等同理加密后替换。
- **备份导出**（src/data_backup.rs）：现在整目录拷贝。加密后导出的自然是密文
  （等于自带加密的备份包，好事）；恢复流程在导入前要求输入密码、验证明文 / 密文格式。
- **`.gwx` 同步包**：本就另有 ZIP 密码，包内数据库届时为密文，链路基本不动；
  注意旧版（明文库）→ 新版（密文库）跨版本同步的兼容提示。
- **升级回退**：加密库无法被旧版应用读取，升级前建议用户跑一次全量备份；
  帮助文档写清楚。

## 六、分期与已知坑

### 第一期：锁屏 + 双密码（纯应用层，不动存储格式）

锁屏界面、密钥句柄、双验证器、诱饵空库（此期诱饵库先用**明文**空库，
 SQLCipher 属于第二期；第一期先把交互和结构立住）、设置页改密 / 停用、
 全量测试。**验收**：真 / 假密码各自进入、数据互不越界、锁屏前后行为一致。

### 第二期：SQLCipher + 配置加密

rusqlite 换 feature、全部开库路径接密钥句柄、迁移向导、`config.json` 与密钥文件
AEAD、`data_backup` 适配。**验收**：565 MB 真实库迁移后检索 / 词表扫描 /
知识库查询全部正常；CI 三平台编译通过。

### 第三期：图片 / 运行目录加密 + 收尾

`images/`、`pi/` 加密，mermaid-cache 挪缓存目录，日志内容清理，使用帮助更新
（威胁模型、忘记密码不可恢复、备份建议）。

### 已知坑（开工前重读本节）

1. **瞬态开库点多**：十几处 `ManuscriptStore::open(manuscript_db_path())`
   分散在后台任务与 AI 服务里，密钥句柄的取用处必须全量排查，漏一处就是
   「解锁后某个后台任务开库失败」。
2. **`cargo test` 大量测试直接开明文库**：测试路径需要一个不加密（或固定测试密钥）
   的旁路，否则几百个测试全要重写。建议 `ManuscriptStore::open` 保持签名不变、
   内部从句柄取 key，测试用 `set_test_config_dir` + 测试句柄。
3. **bundled-sqlcipher-vendored-openssl 的 CI 时间**：三条发布流水线首编都会变慢，
   需真机验证；若 ARM64 容器里编 OpenSSL 出岔子，退路是 `bundled-sqlcipher`
   （链系统 OpenSSL，但麒麟 / Arch 用户机不一定装，慎重）。
4. **WAL 与原子替换**：备份 / 复制加密库前必须 checkpoint 并把 `-wal` 并回主文件
   （或连 `-wal` 一起拷），否则恢复出来缺页。`data_backup.rs` 现有逻辑
   （src/data_backup.rs:144 起）要逐条过一遍。
5. **`.zip-password`、崩溃日志、egui `app.ron`** 这类边角文件容易漏进明文遗漏清单，
   第三期收尾时按 4.3 清单逐文件核对配置目录。
6. **假系统可信度**：假系统的 `config.json` 同样要有合理的默认配置；
   截屏、打印、导出在假系统里都要能用（导出到用户自选目录，不泄露真数据）。

## 七、红线复核

- 本方案**不改 AI 写正文的三条红线**；解锁与密钥句柄属于基础设施层。
- 密钥句柄本身只读不写正文；任何 AI 路径拿不到密码 / 密钥明文，
  Pi 运行目录加密后，Pi 进程读写来源材料走应用层解密后的视图（内存明文，符合口径）。

## 八、第一期实现进度与交接（2026-11-04）

**第一期已完成开发，待真机验收**（真实密码 / 第二密码各自进入、闲置自动锁定、
安装包内真机跑一遍）。与第三节方案的出入：

- **外壳用包裹而不是改名**：没有按原方案把 `GongwenApp` 的 `impl eframe::App`
  改名，而是新增 `src/app/gate.rs` 的 `AppShell` 做外层 `eframe::App`：
  锁屏期它自己画（裸标题栏 + 居中卡片），解锁后把 `egui::Ui` 委托给
  `GongwenApp::update_workspace`。好处是 30 多个 `impl GongwenApp` 文件零改动。
- **数据槽是进程级前缀规则**：`storage::DataSlot::{Real, Decoy}`，诱饵系统全部
  数据文件统一 `alt-` 前缀（`alt-config.json`、`alt-manuscripts.db`、`alt-pi/`、
  `alt-apis/`、`alt-api-secrets.json`……），`slot_file` / `slot_subdir` 两个入口，
  已接线的调用点：config、manuscripts.db、数据备份源、Pi 运行目录 / 技能库 /
  能力快照、apis 目录与密钥文件。诱饵写入物理上不可能落进真文件。
- **vault.dat 恒为双槽定形**：不设第二密码时诱饵槽放随机密钥的哑封套，
  文件形状与设过时完全一致（`seal_shapes_are_constant_whether_or_not_decoy_is_set`
  测试钉死）；解锁错误统一「密码不正确」，界面任何位置不显示第二密码是否已设置，
  设置页只有「设置（覆盖）/ 清除」两个盲操作。
- **首启引导闭环**：未设密码首启进「设置访问密码」卡片（可勾第二密码），
  「暂不设置，直接进入」写跳过标记（`Vault::create("", None, true)`），之后不再
  打扰，可在设置页「应用锁」重新启用；锁文件损坏（`Probe::Broken`）拒绝一切
  解锁，只给「清除并重启」。
- **闲置自动锁定**：`config.auto_lock_minutes`（默认 15，0=永不），
  无输入事件超时即调 `request_lock`——丢掉整个 `GongwenApp`（Drop 释放
  数据库连接与内存）回锁屏；菜单「锁定窗口」手动触发同一条路。
  锁定前先 `cancel_pi_tasks` 停掉 Pi 后台进程。
- **锁屏 UI 主题适配**：全部颜色走 `theme::` 取色，主按钮必须包
  `theme::accent_scope`（`primary_button_widget` 本身无底色的坑已踩过）；
  三套主题样张由 ignored 测试 `gate_samples` 出到 `tmp/gate-*-{unlock,setup}.png`。
- **macOS 标题栏指标**：`configure_native_titlebar` 需要 CreationContext，
  解锁时拿不到，拆出 `configure_native_titlebar_fallback()`（macOS 返回
  固定 88.0、其他平台 None），创建外壳时先配一次、解锁时再兜底。

第二期（SQLCipher）开工前先重读第六节「已知坑」第 1–5 条；第一期代码入口：
`src/vault.rs`（验证器与锁文件）、`src/app/gate.rs`（外壳与锁屏）、
`src/app/vault_settings.rs`（设置页分区）、`src/storage.rs`（数据槽）。

## 九、第二期实现进度与交接（2026-11-05）

**第二期已完成开发，全部 2885 个测试通过，待真机验收**（真实库启用→重启→解锁
自动迁移→检索/词表/知识库正常、改密、设/清第二密码、停用解密、加密库备份）。
与方案第四节的关键出入：

- **DEK 包裹架构**（取代「密码→密钥→一切」）：密码只用于 Argon2id 派生**包裹
  密钥**加解密一把随机的 32 字节**数据密钥（DEK）**；DEK 才是 SQLCipher 库密钥
  与文件 AEAD 密钥。改主密码 = 用新包裹重裹同一把 DEK，数据文件一个不动，
  杜绝「改密把数据锁死」的自锁风险。`vault.dat` 升 v2（槽内加 `wrapped`，
  v1 文件解锁命中时**懒升级**——只补命中那个槽，另一把密码的槽等自己解锁再补，
  不提前作废）。
- **迁移时机在解锁**：设置页「启用」后本会话仍按明文开库（句柄
  `db_active = false`），下次解锁时闸门后台线程做整库迁移
  （`vault::ensure_slot_encrypted`）+ 密封三个敏感文件，完成后才
  `activate_db_encryption()` 进工作区；卡片显示「请稍候，不要关闭应用」。
- **停用 = 解密**：设置页「停用」写 `vault.pending-disable` 标记 + 请求锁定；
  锁屏输一次密码 → 闸门后台把库迁回明文、三个文件解封 → 删锁文件写跳过标记。
  中途崩溃靠标记文件下次启动重进同一条解密路（`cancel_disable` 可取消）。
- **配置与小文件 AEAD**：`config.json`、`.zip-password`、`api-secrets.json`
  整份 `GWA1 ‖ nonce ‖ AES-256-GCM(DEK)`；`ai_services::persistence` 的
  `api-envs.json` / `api-tests.json` 同样密封；密封在**写路径**发生（句柄在就密），
  未解锁读到密封文件按「损坏/默认」处理，不崩。
- **备份快照保持密文**：`data_backup::snapshot_database` 源带 key 开、目标也带
  key 导出——备份包等于自带加密；无句柄（未启用应用锁）时两端明文，与磁盘一致。
- **解锁快进路径（2026-11-05 修订）**：「正在加密现有数据」卡片只在**真的有活干**
  时出现——解锁命中后先跑 `vault::slot_needs_encryption(slot)`（纯文件头嗅探：
  库头 16 字节看 `SQLite format 3` 魔数 + 名单内小文件看 `GWA1` 魔数，不打开
  数据库），该槽位没有明文数据就直接带 key 进工作区，不起后台线程。此前每次
  解锁都显示卡片，是因为判定形态走 `cipher_integrity_check` 全库 HMAC 扫描，
  大库每次解锁都要扫一遍。`database_matches` 也改成了带 key 读 `sqlite_master`
  （只碰第一页），不再全库扫描。
- **两套密码交替解锁互不冲突**：解锁搬运一律显式按命中槽位操作——`begin_work`
  先 `set_current_slot(slot)` 再起线程，库路径与名单文件都走 `slot_file` 映射；
  两套 DEK 各自只管自己槽位的文件（真的库用真 DEK、诱饵 `alt-` 文件用诱饵
  DEK），「诱饵槽有明文文件」也不会让真槽显示搬运卡片
  （`slot_needs_encryption_sniffs_heads_per_slot` 测试钉死）。
- **测试旁路**：无句柄时 `storage::apply_db_key` 空转，全部既有测试照旧跑明文库；
  句柄在测试里是**线程本地**覆盖（`TEST_KEY_HANDLE`，同 `TEST_CONFIG_DIR` 思路），
  并行测试互不清句柄。

### 二期踩过的坑（改这块代码前必读）

1. **`PRAGMA hexkey` 与 `key = 'x'..'` 在这个构建里不是同一把钥匙**：hexkey 把
   解码后的 32 字节当**口令**再走一遍 PBKDF2；`x'hex'` 才是原始密钥，且和
   `ATTACH ... KEY "x'hex'"` 一致。全库统一用 `x'..'`
   （`vault::apply_key_with`、`storage::apply_db_key`），混用会让迁移出的库
   日常打不开。
2. **SQLCipher 不支持 SQLite 在线备份 API**（`sqlite3_backup_init` 直接报错），
   迁移与备份快照都走 `sqlcipher_export`（`vault::export_database`）：
   `ATTACH DATABASE 'path' AS export_dest KEY "x'..'"` →
   `SELECT sqlcipher_export('export_dest')` → `DETACH`。
3. **ATTACH 的密钥必须是 SQL 文本内联的字面 token**：绑定参数传 `x'..'` 会被
   当普通口令派生，导出的库用错密钥（静默成功，读时才炸）。
4. **ATTACH 继承主连接的 openFlags**：主连接不带 `SQLITE_OPEN_CREATE` 时
   建新附库文件直接 CANTOPEN——导出前先建空文件占位。data_backup 的源连接
   因此用 `OPEN_READ_WRITE`（不能 READ_ONLY，附库要写）。
5. **`cipher_integrity_check` 只在发现问题时返回行**（错误描述），全过是
   **零行**——别等 "ok" 行（`database_matches` 曾因此把好的密文库判成损坏）。
6. **v1→v2 懒升级是刻意的**：解锁只补命中槽的 `wrapped`，保证另一把密码
   （第二密码或主密码）不失效；测试断言按这个语义写。
7. **本机构建链**：bundled-sqlcipher-vendored-openssl 要编 OpenSSL，本机 Git
   自带 MSYS perl 缺模块、Cygwin perl 被 Configure 拒；用便携 Strawberry Perl
   （`C:\Users\lisiy\perl`），cargo 命令前
   `export PERL="C:/Users/lisiy/perl/perl/bin/perl.exe"`。CI 的
   windows-latest 自带完整 Strawberry，不受影响。

### 二期代码入口

`src/vault.rs`（DEK、封套、迁移、AEAD、停用标记）、`src/storage.rs`
（`apply_db_key`、`wipe_slot_files`、密封写路径）、`src/app/gate.rs`
（闸门后台工作 `GateWorking`）、`src/app/vault_settings.rs`（启用/改密/
停用接线）、六个开库点（`manuscript.rs` / `knowledge.rs` / `lexicon.rs` /
`lexicon/scan.rs` / `task_assistant/store.rs` / `agent/writing/store.rs`）、
`src/data_backup.rs`（快照）、`src/ai_services/persistence.rs`（JSON 密封）。

### 真机验收清单（二期）

1. 设置页启用密码保护 → 提示「下次解锁自动加密」→ 锁定 → 解锁：出现「请稍候」
   卡片，迁移完成进工作区；`manuscripts.db` 头不是 `SQLite format 3`。
2. 迁移后检索、词表扫描、知识库、任务助手全部正常（SQLCipher 页级透明）。
3. 重启应用输密码直接进（已是密文，不再迁移）。
4. 改主密码：立即生效，数据不动；旧密码失效。
5. 设/换第二密码：旧诱饵数据被擦；第二密码进空白系统；清除后该密码失效。
6. 停用：锁屏输一次密码 → 回到无密码状态，库回到明文（头可读）。
7. 数据备份：启用锁后导出的备份包在**未解锁**的机器上打不开库文件；
   解锁后导入正常。
8. 大库（565 MB 级）迁移耗时与空间占用（临时文件同目录，需双倍磁盘余量）。

## 十、锁屏界面与反复锁定（2026-11-05）

### 实现行为

- 冷启动检测到已启用密码锁时，在创建原生窗口前就指定约 380 × 250 逻辑像素；标题栏只留品牌、
  应用名与关闭按钮。正文只显示「访问密码」、密码框（框内显示/隐藏眼睛按钮）、同宽「解锁」
  按钮和右侧 Enter 提示。首次设置密码仍使用可容纳表单的较大窗口。
- 外壳在 eframe 的本机状态文件中另存 `gongwen_shell_v1`，只含工作区尺寸、位置、最大化/全屏与
  最后使用的主题，不含槽位身份、稿件、密码或密钥。启动小窗关闭不会覆盖工作区尺寸；解锁前也可恢复
  上次主题并跟随 Omarchy 变化。首次升级兼容 eframe 原来的窗口状态，缺省工作区为 1280 × 820。运行中手动锁定或闲置锁定保留当前工作区窗口大小。解锁后工作区先构造，随后
  约 300 毫秒淡出遮罩，期间清空键盘事件、重置指针并以最上层交互区域挡住鼠标。
- 锁定前会保存全部已修改或新建的稿件与配置，不受自动保存开关影响；有任务未结束或保存失败时
  不丢弃工作区，状态栏显示原因。成功后才释放工作区与密钥句柄。
- 锁屏遮罩使用当前主题色，对不含用户信息的抽象三栏与排版节奏生成 160 × 100 底图，再做高斯模糊
  和雾面叠色。它不读取、截图或缓存真实工作区像素。解锁后的工作区仍会
  按目标密码槽的配置重新应用主题。输错密码保持蒙版，输入框清空并恢复隐藏模式。
- 锁定时清空 egui 弹窗、焦点和编辑缓存，禁止持久化 egui 内存；解锁过渡使用已登记的顶层区域，
  关闭 Area 默认淡入以免首帧漏底，并在测量帧直接绘制蒙版。所有蒙版坐标均用窗口内部内容坐标。
  过渡显示的是新构造的目标槽工作区，绝不将上一工作区快照逐渐擦清。
- 外壳只消费一次启动时传入的稿件包；单实例锁和收件队列接收器在每次锁定时从工作区交还给外壳，
  锁屏期间转交的 `.gwx` 请求留在队列，解锁后继续处理。主/诱饵槽仍按命中槽位打开，不显示槽位提示。

### 真机手动验收

1. 启用密码保护并重启：确认启动窗口约 380 × 250，标题栏和四项解锁内容布局紧凑；输入错密码只显示
   「密码不正确」，输入正确密码后恢复桌面窗口位置、大小或最大化状态。
2. 从首次设置界面验证密码设置、确认、第二密码和跳过仍可操作；输入框焦点、Enter 提交及眼睛显隐可用。
3. 在真实工作区打开带正文的稿件、设置弹窗与下拉菜单，手动锁定：确认当前帧不闪回清晰内容，之后只见
   低对比度抽象块和锁屏密码框；拖动或点击遮罩不会操作背后控件。解锁后观察约 300 毫秒淡出，窗口恢复，
   旧弹窗不覆盖工作区。
4. 在锁屏时从第二进程发送 `.gwx`，随后解锁：确认包被接收且未丢失；再锁定/解锁至少两次，确认仍可用。
5. 主密码与诱饵密码交替进入：确认各自配置与稿件不串槽；在锁定前制造保存失败或保留运行任务，确认工作区
   留存且错误清楚可见。

### 已知限制

遮罩模糊的是无用户内容的抽象布局，视觉不逐像素复刻锁定前页面。已入库稿件与活动标签通过现有会话恢复；
临时 PDF 标签、编辑光标与滚动位置仍按现有恢复能力处理，不保留整套活动工作区。Windows、Linux、macOS 的标题栏、窗口位置与锁屏绘制仍需
真机目视检查，特别是高 DPI 和 Omarchy 直角外壳。

### 本轮验证与审查

- 采用 ai-handoff：一个 gpt-6-luna / high 子代理实现，主代理审查实际工作区 diff 后修复。
- 主代理补齐：原生创建前小窗、独立外壳外观持久化、Omarchy 锁屏跟随、锁定保存后的会话记忆、
  错密码保持蒙版、窗口本地坐标与首帧遮罩、清理旧编辑缓存、模糊底图与紧凑排版。
- 针对性测试覆盖关闭自动保存后的新旧稿保存、保存失败保留修改、错误密码保持蒙版、反复交接单实例队列。
- 样张命令：`cargo test --locked gate_samples -- --ignored --test-threads=1`，输出
  `tmp/gate-{claude,dark,paper,square}-{unlock,veil,setup}.png`。square 使用 MDEX 夜墨验证直角外壳；
  Omarchy 实际桌面、原生窗口移动/缩放和 300ms 过渡仍需真机验收。

- 本轮最终结果：`cargo fmt --all -- --check`、`cargo check --locked --all-targets`、
  `cargo clippy --locked --all-targets -- -D warnings` 通过；`lifecycle_tests` 4 项通过；
  `gate_samples` 通过，12 张样张完成并目视检查浅色小窗、深色蒙版、稿纸首设和直角小窗。
  首轮纹理缓存重入锁与矮窗口首设裁切均已修复并复验。未运行全库测试、未发布或提交。
