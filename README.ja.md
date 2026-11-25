<div align="center">

# EasyTrace-rs (`easytrace`)

**純 Rust 製・高速 PathFinder 交渉型輻輳（Negotiated-Congestion）PCB オートルーター**  
*JLCEDA Pro（EasyEDA Pro）の基板くり抜き・NPTH ボス回避、KiCad、Specctra DSN/SES にネイティブ対応*

[**English**](README.md) | [**简体中文**](README.zh-CN.md) | [**日本語**](README.ja.md)

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Pure_Rust-2021_Edition-orange.svg)](Cargo.toml)
[![JLCEDA Pro](https://img.shields.io/badge/JLCEDA_Pro-v2.3%2B_%2F_v4.1%2B-00b96b.svg)](extension/easytrace-jlceda-ext)

</div>

---

**EasyTrace-rs** は、**100% 純 Rust**（外部クレートは `serde` と `serde_json` のみ・Python/C ランタイム依存ゼロ・数秒でビルド完了）で実装されたスタンドアロン高速 PCB 自動配線エンジンです。`0.35 mm` ピッチ QFN-68、`0.40 mm` ピッチ WLCSP-9、大型バッテリー開口部（Cutout）、タクトスイッチの NPTH 位置決めボスが混在する高密度 4〜6 層ウェアラブル基板の実戦から生まれ、**JLCEDA Pro のネイティブ DRC 違反ゼロの 0°/45°/90°（オクティリニア）配線**を数秒〜数分で生成します。

---

## JLCEDA Pro（嘉立創EDAプロ版）は AI Bridge なしで外部サードパーティ製オートルーターを使えますか？

**はい、使えます！人間の設計者向けに AI Bridge 不要の 2 つの方法**があり、さらに AI エージェント向けのヘッドレス閉ループ方式も備えています：

| ワークフロー | AI Bridge | 拡張機能 | 基板開口部・NPTH ボスの保持 | 対象ユーザー |
|---|:---:|:---:|:---:|---|
| **方法 1：ワンクリック拡張機能 + `easytrace serve`** *（推奨）* | **不要** | 要（`.eext` 同梱） | **100% 完全保持**（`prohibitedRegions` + 1oz/2oz ルール） | JLCEDA Pro デスクトップ版 & Web 版 (`pro.lceda.cn`) の一般ユーザー |
| **方法 2：Specctra `.dsn` -> `.ses` ファイル連携** | **不要** | **不要** | 標準 Keepout のみ（下記 JLCEDA `.dsn` 注意事項参照） | JLCEDA Pro、**KiCad**、**Altium Designer** |
| **方法 3：AI エージェント向け Bridge + 閉ループ DRC** | 要 | 不要 | **100% 完全保持** + ネイティブ DRC 自動検証 | Claude Code、Antigravity、Cursor、Codex などの AI エージェント |

---

## ビルドとインストール

```bash
git clone https://github.com/eggfly/EasyTrace-rs.git
cd EasyTrace-rs
cargo build --release

# 実行バイナリ：
./target/release/easytrace --help
```

---

## 3 つの配線ワークフロー

### 方法 1：AI Bridge 不要・ワンクリックローカル拡張機能（JLCEDA Pro 推奨）

AI ブリッジの設定も `.dsn` エクスポートの制限も気にせず、通常の GUI 操作だけでワンクリック自動配線が可能です：

1. **ターミナルで軽量ローカルサーバーを起動**（標準ライブラリ `std::net::TcpListener` のみ使用）：
   ```bash
   ./target/release/easytrace serve
   # http://127.0.0.1:3789 で待機（CORS / Private Network Access 対応によりデスクトップ版・Web 版の両方で動作）
   ```
2. **同梱の JLCEDA Pro 拡張機能をインストール**（初回のみ）：
   - JLCEDA Pro（デスクトップアプリまたは Web 版 `https://pro.lceda.cn`）を開きます。
   - **`設定 -> 拡張機能 -> 拡張機能をインストール...`** から [`dist/easytrace-jlceda.eext`](dist/easytrace-jlceda.eext) を選択します（`python3 extension/easytrace-jlceda-ext/build_eext.py` でいつでも再ビルド可能）。
3. **上部メニューの `EasyTrace-rs -> Auto Route Current PCB...`**（または **`Quick Auto Route (1-Click)`**）をクリック：
   - 拡張機能が `await eda.pcb_ManufactureData.getAutoRouteJsonFile()` を呼び出し、**基板内部の開口部、タクトスイッチ `SW*` の NPTH スロットボス、MEMS マイク音響穴（`prohibitedRegions`）、裏面部品の `MIRROR_Y` 反転座標、1oz/2oz クリアランスルールを 100% 取得**します。
   - `http://127.0.0.1:3789/route-jlceda` で高速配線を実行し、`pcb_PrimitiveLine` と `pcb_PrimitiveVia` を直接キャンバスへ書き込みます。

---

### 方法 2：プラグイン不要の標準ファイル連携（`Specctra .dsn -> .ses`）

**KiCad**、**Altium Designer**、および拡張機能を入れずに **JLCEDA Pro** を使いたい場合の標準フローです：

1. **`.dsn` ファイルをエクスポート**：
   - **JLCEDA Pro**：`ファイル -> エクスポート -> 自動配線ファイル (DSN)...`
   - **KiCad PCB エディター**：`ファイル -> エクスポート -> Specctra DSN...`
2. **`easytrace dsn` で配線実行**：
   ```bash
   ./target/release/easytrace dsn board.dsn -o board.ses --skip GND --power VBAT,VDD_3V3
   ```
3. **`.ses` セッションファイルをインポート**：
   - **JLCEDA Pro**：`ファイル -> インポート -> 自動配線セッションファイル (SES)...`
   - **KiCad PCB エディター**：`ファイル -> インポート -> Specctra Session...`

> [!WARNING]
> **JLCEDA Pro v4.1 の `.dsn` エクスポーターに関する既知の制限：**  
> JLCEDA Pro 標準の `.dsn` エクスポートは、基板内部のくり抜き穴（Cutout）や部品フットプリント内の NPTH ボス（Slot Region）に対する `0.30 mm` スロットクリアランスを `keepout` として出力せず、`0.10 mm` クリアランスの未接続パッドとして出力してしまいます。内部開口部やスイッチの位置決め穴がある JLCEDA Pro 基板では、**すべての `prohibitedRegions` を正確に保持できる方法 1（`easytrace serve` + `.eext`）または `easytrace jlceda` の使用を強く推奨します。**

CLI から直接 JLCEDA の `AutoRouteJson` を配線することも可能です（`--ses` で `.ses` も同時出力可能）：

```bash
./target/release/easytrace jlceda autoroute.json -o routes.json --ses routes.ses --skip GND
```

---

### 方法 3：AI エージェント向けヘッドレス Bridge & 閉ループ DRC 検証

プロジェクト非依存の自動化スクリプト（[`scripts/`](scripts)）、[`SKILL.md`](SKILL.md)、[`AGENTS.md`](AGENTS.md) を同梱しており、**Claude Code / Antigravity / Cursor / Codex** などの AI エージェントが自律的に配線と **DRC エラー 0 件**の検証を完結できます：

```bash
# 1. Bridge 経由で AutoRouteJson 取得 -> Rust 高速配線 -> PCB へ一括反映：
python3 scripts/bridge_route.py \
  --bridge-url http://127.0.0.1:49620 \
  --pcb-uuid <PCB_UUID> \
  --mode incremental \
  --skip GND \
  --power VBAT,VDD_3V3 \
  --save-dir ./run_artifacts

# 2. オフライン幾何 DRC チェック & レイヤー別 SVG 出力（pip パッケージ一切不要）：
python3 scripts/drc_check.py --autoroute ./run_artifacts/autoroute.json --routes ./run_artifacts/routes.json
python3 scripts/render_svg.py --autoroute ./run_artifacts/autoroute.json --routes ./run_artifacts/routes.json \
  -o ./run_artifacts/board_routes.svg --split-layers

# 3. JLCEDA Pro ネイティブ DRC 実行（--reopen で新規ビアのネット接続認識を確定）：
python3 scripts/drc_check.py \
  --bridge-url http://127.0.0.1:49620 \
  --pcb-uuid <PCB_UUID> \
  --project-uuid <PROJECT_UUID> \
  --reopen
```

---

## アーキテクチャの特長

1. **膨張ゼロの正確な中点対角障害物マップ（`st_ortho` + `st_mid`）**：
   - 従来のグリッドルーターが対角移動に課していた `h * sqrt(2) / 2`（直径 `+0.071 mm`）の仮想膨張を排除。グリッド頂点 `st_ortho` と半セルずらしたセル中心 `(x0 + 0.5h, y0 + 0.5h)` `st_mid`（NE/SW と NW/SE 両対角線の正確な中点）を併用し、`0.35 mm` QFN や `0.40 mm` WLCSP-9 中央ボールからの引き出しを実現します。
2. **動的輻輳マップの直交/対角分離（`dy_ortho` vs `dy_diag`）— 配線チャネル容量 +20%**：
   - 平行な直交 `0.127 mm` 配線のピッチを 6 セル（`0.300 mm`）から **5 セル（`0.250 mm`、銅箔間隔 `0.123 mm >= 0.102 mm`）** へ短縮し、全層のバス配線密度を 20% 向上させます。
3. **同一ネットのパッド近傍ファンアウトビア解禁（Same-Net Via-Near-Pad Unlock）**：
   - 異ネット間には完全な `vcl + vr`（`~0.355 mm`）を確保しつつ、同一ネットのパッドに対してはドリル径 `via_hole_d / 2.0 + 0.02 mm` のみを `0x4000_0000 | net` でマスクすることで、はんだ吸い上がりを防ぎながら QFN/0402 パッド直近へのファンアウトビア配置を可能にします。
4. **加算型 VPR PathFinder + ブランチ単位 Steiner リップアップ + 単調レスキューパス**：
   - 多ピンネットのうち競合しているブランチのみを剥がして健全なブランチを温存し、最終ハードパス後に残った未接続ネットに対して 1〜3 本の他ネットブランチのみを退避・再配線する **Targeted Rescue Pass**（総接続数が厳密に増加した場合のみコミット）を実行します。
5. **電源配線の自動パッドネックダウン & `orig_edges` 検証付きオクティリニア平滑化**：
   - `0.200 mm` 電源ネットはパッド周囲 `0.55 mm` 以内で自動的に `0.127 mm`（`5 mil`）へネックダウン（6 層 `3.5 mil` ルール基板では `--min-width 0.095` も選択可能）。ドリル穴間隔 `hole2Hole >= 0.305 mm`、スロット対配線 `0.31 mm` / スロット対ビア `0.19 mm`、`orig_edges` チェック付き 0°/45°/90° 引き締め平滑化を標準装備しています。

---

## 実測ベンチマーク比較（中央開口部付き高密度 4〜6 層スマートウォッチ基板・226 接続）

| オートルーター | 実行時間 | 配線完了率 | JLCEDA ネイティブ DRC 違反 | 備考 |
|---|---:|---:|---:|---|
| **EasyTrace-rs (`easytrace`)** | **12 秒 – 95 秒** | **97.3% – 100%** | **0 件** | 開口部・`SW*` NPTH ボス・`hole2Hole >= 0.305 mm`・`0.127 mm` 最小線幅を完全遵守 |
| **Java FreeRouting v2.x** (`.dsn`/`.ses`) | 4 分 – 15 分以上 | プラトーで停滞 | 複数発生 | パッド付近で `< 0.127 mm` の細線を生成、`.dsn` 経由ではスロット間隔が欠落 |
| **JLCEDA Pro 内蔵オートルーター** | 14 分以上（UI 占有） | 高密度部 < 15% | 0–5 件 | エディター UI をブロックし、`0.35 mm` QFN 周辺や大型開口部の迂回で停止 |
| **Python グリッド迷路ルーター** | 6 時間以上 | ~55% | クリアランス違反あり | 交渉型リップアップがなく高密度ファンアウトでデッドロック |

---

## ライセンス

[MIT License](LICENSE) の下で公開されています。
