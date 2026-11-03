# 變更日誌

## [Unreleased]

### 修正

- 經 daemon 快取解析直鏈時會先檢查到期時間:已過期的鏈結立即
  刷新一次,不再原樣交給 ffprobe。掃描後隔一段時間才觸發的片尾
  偵測,不會再因過期的簽名 URL 而靜默失敗。

## [0.2.2] — 2026-11-03

### 修正

- 視覺片尾偵測對同一檔案跑了兩次 ffmpeg:第一輪的輸出直接丟棄,
  之後才跑真正的定寬輪。對網路 strm 目標,等於每次片尾掃描的
  下載與解碼成本翻倍。

### 變更

- ffprobe 加上 `-rw_timeout 50000000`(50 秒讀取停滯上限):
  CDN 連線卡死時 50 秒內報錯,不再等到外層兩分鐘程序逾時。
- ffprobe 協議白名單補上 `crypto` 與 `httpproxy`(部分帶簽名
  query 的 CDN 缺 `crypto` 會交握失敗)。
- 日誌衛生:ffmpeg/ffprobe 錯誤與 stderr 尾段中出現的 URL
  一律剝除 query string 後再輸出,CDN token 不再進掃描日誌。
- Plex schema 門檻改為只比對 `schema_migrations` 主版本:Plex
  的修板遷移(如 `.232`)不再停用寫入;主版本不同仍拒寫。

### 安全

- runtime daemon 的 unix socket 改為顯式 `0o660` 權限建立,
  不再繼承服務 umask。

### 效能

- 掃描器在整個程序生命週期重用同一個 daemon client,不再
  每個 item 重建 HTTP client。
- 直鏈快取與死鏈帳本的 SQL 語句改走 SQLite prepared statement
  快取。

## [0.2.1] — 2026-11-03

### 新增

- 片頭偵測:跨集音訊指紋比對(chromaprint),對同季集數取指紋、
  互相關比對後寫入共識片頭標記;支援片頭在不同集數從不同位置
  開始的冷開場劇集。
- 非黑底片尾偵測:當黑窗啟發式找不到(畫面上疊字)時,第二輪
  以 0.5 fps 分析邊緣密度,找出最長的持續文字段。
- 掃描輪內死鏈自癒:探測撞上游 4xx 時,每個目標整輪僅去重刷新
  一次;持續失效的目標進入退避,不再重複探測。
- 雲端硬碟保護:每網域熔斷器(3 次失敗、3 分鐘冷卻翻倍至 30 分
  鐘)、死鏈台帳(1/2/4/7 天退避)、每小時掃描配額。
- s6 服務整合:scanner/transcoder 軟鏈與連結快取 daemon 在容器
  重啟後自動恢復。
- HDR 徽章支援:從視訊流的 transfer characteristics 寫入 `color_trc`。
- 完整的視訊流 `extra_data` 集(檔案、位元深度、色度、色彩空間、
  幀率、掃描類型),用戶端不再顯示 "Unknown"。
- `assistant intro --section N` 指令用於片頭偵測。
- 開發工具:`strmock`(假雲端硬碟)、`plex-truth.sh`(fixture
  採集)、`bin-swap.sh`(本地輔助)。

### 變更

- 專案從 Go 完整重寫為 Rust(單一靜態二進位、musl、交叉編譯
  不需宿主工具鏈)。
- 片尾偵測僅掃描最後 10 分鐘、0.5 fps / 320px(一小時的 4K
  影集解碼量約減少 40 倍)。
- 串流語言改存 ISO 639-1 兩字碼("eng" → "en"、"chi" → "zh"),
  與 Plex 官方寫入一致。

### 修復

- 視訊串流不再在用戶端顯示 "Unknown (HEVC)"——檔案名與顯示
  標題改從 `extra_data` 推導。
- NULL 語言與視訊位元率值不再讓 Plex 的 SQL 層在完整元資料
  視圖上崩潰。
- 修復 UA 綁定雲端連結(115 CDN)上的轉碼器崩潰:包裝器現在
  在每個換入的輸入前注入解析時的使用者代理。
- 修復 EAE 音訊解碼器逾時後,孤兒轉碼工作階段堵住並發限制
  的問題。

## [0.2.0] — 2026-07-23

### 新增

- 片尾標記(啟發式黑窗偵測)。
- 安裝期掃描偏好(FSEvent)。
- s6 開機服務。
- 自建 Docker 映像(多階段建置、linuxserver/plex 底層)。

## [0.1.0] — 2026-07-17

### 新增

- 首次發布:scanner/transcoder 接管、STRM 分析(ffprobe)、
  SQLite 直寫、持久連結快取、雲端硬碟保護(熔斷器、台帳、
  配額)、install/verify/revert 生命週期。

[0.2.1]: https://github.com/PlexAssistant/PlexAssistant/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/PlexAssistant/PlexAssistant/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/PlexAssistant/PlexAssistant/releases/tag/v0.1.0
