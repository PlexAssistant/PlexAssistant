# 變更日誌

## [未發布]

### 修復

- 掃描整個 section 後的片頭偵測會走遍整個媒體庫的每一季,而不只是該
  section 的季。
- 掃描時的解析與刷新被記成播放讀取,每日雲端用量因此算進了掃描的快取
  命中和檔案大小。

### 變更

- 片尾偵測只對檔案做一次 ffprobe。原本黑場偵測會在其他偵測前自己再跑
  一次。
- 掃描每個項目只開一次 assistant 快取,而不是每次查詢都開。

## [0.4.5] — 2026-11-03

### 修復

- 掃描的整合測試缺了查季時要用的欄位,分析成功後 `cargo test` 仍失敗,
  發布用的映像沒有建出來。

## [0.4.4] — 2026-11-03

### 新增

- 音軌和字幕的選擇按帳號、按劇記住。再次分析一個 part 時,選擇會改指到
  新的串流列;同一部劇之後的單集,若這個帳號還沒選過,會套用同一個選擇。
  改寫前的串流 id 會留著。`assistant tracks restore` 把它們寫回去,並忘掉
  這個選擇。
- 雲端讀取按網域、按天記下來:讀了幾次、多少位元組、其中幾次不是從頭
  讀。掃描的探測算一個位元組。播放算解析時得知的大小,輸入前有 `-ss`
  算一次跳讀。配額和熔斷仍只數請求次數,播放仍不計入兩者。

## [0.4.3] — 2026-11-03

### 變更

- 片尾取樣先把畫面寬度縮到 320、高度按比例,評分時才補成正方形。
  在縮放裡直接補黑邊會把 16:9 的畫面擠扁,窗口落在兩秒一次的取樣點上。
  同一段合成 16:9 片尾,補黑邊是 20.0–60.0 秒,按比例是 21.5–58.5 秒。

### 新增

- 片頭偵測跟著掃描走。一季至少有兩集 strm 時,分析完會做片頭比對;
  已經記過片頭的單集不會再取指紋。`assistant intro` 仍可單獨跑同一套偵測。

## [0.4.2] — 2026-11-03

### 新增

- 沒能分析成功的 strm part 會單獨記一筆:試過幾次,以及失敗是永久的
  還是先別再試。永久失敗的 part 不再重試。直鏈的失敗仍走連結帳本。
- 打開詳情頁會補上還沒有媒體資訊的 strm。Plex 只在掃描時要求分析,
  它認為已經分析過的項目會一直空著,直到下一次掃描。
- 同一份檔案按時長和大小辨認,不再按 strm 網址。換路徑、換 strm、
  重新加入的同一份檔案直接套用已存的分析結果,不再重新探測。片頭
  與片尾偵測用同一組時長和大小,簽名網址換過也不會重測。

## [0.4.1] — 2026-11-03

### 變更

- 掃描一個媒體庫時同時探測三個項目。結果仍逐個寫入,解析直鏈的
  請求依舊走 daemon 的配額。

## [0.4.0] — 2026-11-03

### 新增

- 片頭與片尾偵測會記下自己寫過的標記。內容沒有變化的項目掃描時
  直接跳過,手動設定的標記不會被之後的掃描覆蓋。`assistant marker`
  可列出這些記錄,也能設定、重試或停止某一筆。
- 片尾偵測會讀取文字周圍的框,用來把滾動字幕的起止從兩秒一次的
  取樣點移開。靜止不動的字幕維持原樣。

## [0.3.1] — 2026-11-03

### 修正

- 無法成功的解析不再走死鏈退避。無法解析的 URL,或上游回應
  404、410,會一直跳過,直到之後某次成功清除該筆記錄。拒絕、
  逾時、重導向環仍走 1/2/4/7 天退避。其他原因的探測失敗也同樣
  記帳,不再每次掃描都重打。

## [0.3.0] — 2026-11-03

### 新增

- 片尾偵測可以使用 Plex 已經存放的模型(掃描器目錄下的
  `model_v1.pb`)。分數格超過門檻的比例夠高,該幀就算有文字;
  標記取連續至少 15 秒的最長一段。
- `ASSISTANT_CREDITS` 選擇偵測器:`auto`(預設)、`heuristic`、
  `model`。`model` 在檔案缺失或無法執行時不寫標記。

### 變更

- 預設片尾鏈改為黑窗,然後是已存放的模型,最後才是邊緣偵測。
  設定 `ASSISTANT_CREDITS=heuristic` 可維持 0.2.2 的鏈。

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

[0.4.5]: https://github.com/PlexAssistant/PlexAssistant/compare/v0.4.4...v0.4.5
[0.4.4]: https://github.com/PlexAssistant/PlexAssistant/compare/v0.4.3...v0.4.4
[0.4.3]: https://github.com/PlexAssistant/PlexAssistant/compare/v0.4.2...v0.4.3
[0.4.2]: https://github.com/PlexAssistant/PlexAssistant/compare/v0.4.1...v0.4.2
[0.4.1]: https://github.com/PlexAssistant/PlexAssistant/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/PlexAssistant/PlexAssistant/compare/v0.3.1...v0.4.0
[0.3.1]: https://github.com/PlexAssistant/PlexAssistant/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/PlexAssistant/PlexAssistant/compare/v0.2.2...v0.3.0
[0.2.2]: https://github.com/PlexAssistant/PlexAssistant/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/PlexAssistant/PlexAssistant/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/PlexAssistant/PlexAssistant/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/PlexAssistant/PlexAssistant/releases/tag/v0.1.0
