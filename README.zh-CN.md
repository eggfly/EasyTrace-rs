<div align="center">

# EasyTrace-rs (`easytrace`)

**纯 Rust 高速协商拥塞（PathFinder）PCB 自动布线引擎**  
*原生支持嘉立创 EDA 专业版（JLCEDA Pro）板内挖槽 / NPTH 定位柱避让、KiCad 与 Specctra DSN/SES*

[**English**](README.md) | [**简体中文**](README.zh-CN.md) | [**日本語**](README.ja.md)

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Pure_Rust-2021_Edition-orange.svg)](Cargo.toml)
[![JLCEDA Pro](https://img.shields.io/badge/嘉立创EDA专业版-v2.3%2B_%2F_v4.1%2B-00b96b.svg)](extension/easytrace-jlceda-ext)

</div>

---

**EasyTrace-rs** 是一款 **100% 纯 Rust** 编写（仅依赖 `serde` 与 `serde_json`，零 Python / C 运行时依赖，数秒即可完成编译）的独立高速 PCB 自动布线器。它诞生于高密度 4/6 层智能穿戴主板的实战打磨（`0.35 mm` 间距 QFN-68、`0.40 mm` 间距 WLCSP-9、板中央大尺寸电池开窗、侧键 `SW*` 非金属化定位柱孔），可在数十秒至数分钟内输出 **嘉立创 DRC 零违规的 0°/45°/90° 八向走线**。

---

## 嘉立创 EDA 专业版（JLCEDA Pro）不装 AI Bridge 能用第三方外部自动布线器吗？

**完全可以！而且提供两种面向普通人类工程师的零门槛方式**（外加第 3 种面向 AI Agent 的无头闭环工作流）：

| 工作流 | 需要 AI Bridge？ | 需要安装扩展？ | 保留板内挖槽与 NPTH 定位柱？ | 适用场景 |
|---|:---:|:---:|:---:|---|
| **方式一：一键本地扩展 + `easytrace serve`** *（强烈推荐）* | **不需要** | 需要（仓库自带 `.eext`） | **100% 完整保留**（`prohibitedRegions` + 1oz/2oz 规则） | 嘉立创 EDA 专业版桌面客户端 & 网页版 `pro.lceda.cn` |
| **方式二：Specctra `.dsn` -> `.ses` 标准文件交换** | **不需要** | **不需要** | 标准禁布区保留（嘉立创导出限制见下方提示） | 嘉立创 EDA 专业版、**KiCad**、**Altium Designer** |
| **方式三：AI Agent 无头 Bridge + 闭环 DRC 验证** | 需要 | 不需要 | **100% 完整保留** + 原生 DRC 自动签核 | Claude Code、Antigravity、Cursor、Codex 等智能体 |

---

## 快速编译与安装

```bash
git clone https://github.com/eggfly/EasyTrace-rs.git
cd EasyTrace-rs
cargo build --release

# 编译产物：
./target/release/easytrace --help
```

---

## 三种布线工作流详解

### 方式一：零 Bridge 人类工程师一键本地扩展（嘉立创 EDA 首选推荐）

无需配置任何 AI 桥接工具，也不会踩 `.dsn` 导出丢失开槽间距的坑，只需两步即可一键布线：

1. **在终端启动轻量级本地布线服务**（基于标准库 `std::net::TcpListener`，零臃肿依赖）：
   ```bash
   ./target/release/easytrace serve
   # 监听 http://127.0.0.1:3789（内置 CORS 与 Private Network Access 头，同时支持客户端与网页版 pro.lceda.cn）
   ```
2. **首次安装仓库自带的扩展包**：
   - 打开嘉立创 EDA 专业版（桌面端或网页版 `https://pro.lceda.cn`）。
   - 依次点击 **`设置 -> 扩展 -> 安装扩展...`**，选择本仓库预打包好的 [`dist/easytrace-jlceda.eext`](dist/easytrace-jlceda.eext)（也可随时运行 `python3 extension/easytrace-jlceda-ext/build_eext.py` 重新打包）。
3. **点击顶部菜单 `EasyTrace-rs -> 自动布线面板...`**（或 **`一键极速布线`**）：
   - 扩展直接调用原生 `await eda.pcb_ManufactureData.getAutoRouteJsonFile()`，**100% 捕获板内电池挖槽、轻触开关 NPTH 塑胶柱孔、MEMS 麦克风声孔（`prohibitedRegions`）、底层元件 `MIRROR_Y` 镜像坐标及 1oz/2oz 安全间距表**。
   - 自动发送至本地 `http://127.0.0.1:3789/route-jlceda` 高速求解，并将生成的走线（`pcb_PrimitiveLine`）与过孔（`pcb_PrimitiveVia`）直接写回当前 PCB 画布！

---

### 方式二：零插件通用标准文件交换（`Specctra .dsn -> .ses`）

适用于 **KiCad**、**Altium Designer** 以及不想安装任何扩展的 **嘉立创 EDA 专业版** 用户：

1. **导出 `.dsn` 自动布线文件**：
   - **嘉立创 EDA 专业版**：`文件 -> 导出 -> 自动布线文件 (DSN)...`
   - **KiCad PCB 编辑器**：`文件 -> 导出 -> Specctra DSN...`
2. **运行 `easytrace dsn`**：
   ```bash
   ./target/release/easytrace dsn board.dsn -o board.ses --skip GND --power VBAT,VDD_3V3
   ```
3. **导回 `.ses` 会话文件**：
   - **嘉立创 EDA 专业版**：`文件 -> 导入 -> 自动布线会话文件 (SES)...`
   - **KiCad PCB 编辑器**：`文件 -> 导入 -> Specctra Session...`

> [!WARNING]
> **嘉立创 EDA 专业版 v4.1 原生 `.dsn` 导出器的已知局限：**  
> 嘉立创 EDA 导出 `.dsn` 时，不会将多层填充挖槽（Cutout）和封装内部非金属化定位孔（Slot Region）导出为带 `0.30 mm` 槽间距规则的 `keepout`，而是导出为仅 `0.10 mm` 间距的悬空焊盘，导致外部 `.dsn` 布线器（含 FreeRouting）贴着挖槽边缘走线而触发嘉立创 DRC `Slot Region` 报错。**若板内含有挖窗、异形槽或侧键定位孔，强烈推荐使用方式一（`easytrace serve` + `.eext`）或 `easytrace jlceda` 子命令！**

命令行直接布线原生 `AutoRouteJson`（可同时输出 `.ses` 文件）：

```bash
./target/release/easytrace jlceda autoroute.json -o routes.json --ses routes.ses --skip GND
```

---

### 方式三：面向 AI Agent 的无头 Bridge 闭环布线与 DRC 签核

仓库内置完全工程无关的脚本集（[`scripts/`](scripts)）、[`SKILL.md`](SKILL.md) 与 [`AGENTS.md`](AGENTS.md)，方便 **Claude Code / Antigravity / Cursor / Codex** 全自动完成「导出 -> 布线 -> 写回 -> 铺铜 -> 原生 DRC 零违规验证」闭环：

```bash
# 1. 通过 Bridge 导出 AutoRouteJson、调用纯 Rust 引擎布线并安全分块写回 PCB：
python3 scripts/bridge_route.py \
  --bridge-url http://127.0.0.1:49620 \
  --pcb-uuid <PCB_UUID> \
  --mode incremental \
  --skip GND \
  --power VBAT,VDD_3V3 \
  --save-dir ./run_artifacts

# 2. 离线几何 DRC 预检 + 多层 SVG 矢量走线图生成（仅用 Python 标准库，零第三方 pip 依赖！）：
python3 scripts/drc_check.py --autoroute ./run_artifacts/autoroute.json --routes ./run_artifacts/routes.json
python3 scripts/render_svg.py --autoroute ./run_artifacts/autoroute.json --routes ./run_artifacts/routes.json \
  -o ./run_artifacts/board_routes.svg --split-layers

# 3. 调起嘉立创原生 DRC 引擎终检（--reopen 自动重开标签页以激活新建过孔的连通性）：
python3 scripts/drc_check.py \
  --bridge-url http://127.0.0.1:49620 \
  --pcb-uuid <PCB_UUID> \
  --project-uuid <PROJECT_UUID> \
  --reopen
```

---

## 核心算法与工程亮点

1. **零虚胖精确中点斜向障碍图（`st_ortho` + `st_mid`）**：
   - 传统网格布线器对 45° 斜步统一外扩 `h * sqrt(2) / 2`（直径虚胖 `0.071 mm`），会直接堵死 `0.35 mm` QFN 引脚通道和 `0.40 mm` WLCSP-9 中心球斜向扇出咽喉（真实净空 `0.357 mm`）。
   - EasyTrace-rs 同时在网格顶点 `st_ortho` 与半格偏移中心 `(x0 + 0.5h, y0 + 0.5h)` `st_mid`（恰好同时是 NE/SW 与 NW/SE 两条对角线的精确中点！）采样静态间距，彻底消除对角线虚胖。
2. **动态拥塞图正交/斜向分离（`dy_ortho` vs `dy_diag`）—— 走线通道容量提升 +20%**：
   - 平行正交 `0.127 mm` 走线中心距从 6 格（`0.300 mm`）压缩至 **5 格（`0.250 mm`，铜皮净距 `0.123 mm >= 0.102 mm`）**，所有信号层总线通道密度提升 20%；过孔图 `vst` / `vdy` 采用精确顶点间距，不叠加斜向余量。
3. **同网络扇出过孔贴焊盘解锁（Same-Net Via-Near-Pad Unlock）**：
   - 在 `vst` 上用正常网络号绘制完整 `vcl + vr`（`~0.355 mm`）保护异网间距，同时仅对 `via_hole_d / 2.0 + 0.02 mm` 绘制 `0x4000_0000 | net` 钻孔禁区——既允许同网络扇出过孔紧贴 QFN/0402 焊盘趾部打孔，又保证钻孔边缘严格留在 SMD 焊盘外侧杜绝漏锡。
4. **加性 VPR PathFinder 协商拥塞 + 分支级 Steiner 拆线重布 + 单调救援通道（Rescue Pass）**：
   - 采用纯加性 VPR 代价函数与受控压力上限（`pres <= 28.0`、加权 A* `1.32x`、多层板内层奇偶 H/V 正交方向偏置），每次迭代**仅拆除存在冲突的单个 Steiner 分支**，完整保留多引脚网络的干净分支。
   - 非破坏性单分支合法化后自动执行**定向拆线救援（Targeted Rescue Pass）**：以 `pres = 200, no_hist = true` 寻找最少受害分支路径（仅置换 1–3 条外网分支并以 `hard: true` 重布），仅在全板合法连通数严格增加时提交，否则无损回滚。
5. **电源线 B20 焊盘口自动缩颈 + `orig_edges` 安全八向拉直平滑**：
   - `0.200 mm` 电源网络在距焊盘 `0.55 mm` 范围内自动缩颈为 `0.127 mm`（`5 mil`）顺畅进出细间距焊盘；若板厂规则允许 6 层板 `3.5 mil` 线宽，可显式开启 `--min-width 0.095` 咽喉自适应缩颈。
   - 严格执行异网与同网过孔钻孔间距 `hole2Hole >= 0.305 mm`、独立挖槽走线间距（`0.31 mm`）与挖槽过孔间距（`0.19 mm`），并在 `smooth_net` 中逐边校验 `orig_edges` 防止 90° 拐角倒角越界。

---

## 实测基准对比（高密度 4–6 层圆屏穿戴主板，含中央电池大开窗，226 条连接）

| 自动布线器 | 运行耗时 | 连通完成率 | 嘉立创原生 DRC 违规数 | 实测表现说明 |
|---|---:|---:|---:|---|
| **EasyTrace-rs (`easytrace`)** | **12 s – 95 s** | **97.3% – 100%** | **0 违规** | 严格遵守挖槽、`SW*` 定位孔、`hole2Hole >= 0.305 mm` 与 `0.127 mm` 线宽下限 |
| **Java FreeRouting v2.x** (`.dsn`/`.ses`) | 4 – 15+ 分钟 | 卡在平台期 | 多处违规 | 会在焊盘口把线宽缩到 `< 0.127 mm`，且 `.dsn` 丢失嘉立创挖槽间距 |
| **嘉立创 EDA 内置自动布线** | 14+ 分钟（锁死界面） | 密集区 < 15% | 0–5 | 长时间占住 GUI 文档，在 `0.35 mm` QFN 与中央开窗绕行处极易卡死 |
| **Python 网格迷宫布线脚本** | > 6 小时 | ~55% | 间距 `< 0.152 mm` | 缺乏协商式拆线重布，高密度扇出区易锁死 |

---

## 开源许可

本项目采用 [MIT License](LICENSE) 开源许可证发布。
