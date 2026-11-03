# PlexAssistant

[English](README.md) | 正體中文

讓 Plex 原生支援 `.strm`：項目會顯示時長、解析度與音軌等媒體資訊，可以直接播放，也不會因為反覆解析連結而讓雲端硬碟帳號被限速。

## 運作方式

Plex 把 `.strm` 當成 35 位元組的文字檔：介面上沒有任何媒體資訊，每次播放還會用不固定的 User-Agent 重新解析裡面的 URL，這正是雲端硬碟限速的常見原因。

PlexAssistant 取代 Plex 啟動的兩支程式，並在旁邊執行一個常駐服務：

- **Scanner**：解析 strm 目標，用解析時的同一個 User-Agent 以 ffprobe 探測直鏈（115 等 CDN 會把連結綁定在 User-Agent 上），再把時長、編碼、解析度、HDR 與字幕語言寫進媒體庫，欄位與官方掃描器一致。打開詳情頁時，也會補上還沒有媒體資訊的項目。重新分析後，帳號選過的音軌與字幕會保留，並套用到同一部劇的後續單集。
- **Transcoder**：播放時把 strm 換成快取的直鏈，再交給原本的轉碼器。連結在播放中失效時，會立即刷新並重試一次。
- **常駐服務**：保存連結快取，每個目標只解析一次，失效才刷新。持續失效的目標依 1、2、4、7 天退避。掃描觸發的解析受每個網域的熔斷器與每小時配額限制，播放不受限。每個網域每天的讀取次數、位元組與跳讀次數會另外記錄。

## 快速開始

1. 下載 [`compose.yaml`](compose.yaml)：

   ```bash
   curl -O https://raw.githubusercontent.com/PlexAssistant/PlexAssistant/main/compose.yaml
   ```

2. 依檔案中的註解修改 strm 資料夾路徑、`PUID`、`PGID` 與 `TZ`；首次啟動可填入 `PLEX_CLAIM`。
3. 啟動：`docker compose up -d`
4. 開啟 `http://<主機>:32400`，新增指向 `/media` 的媒體庫。掃描完成後就會出現媒體資訊。

已經在使用 `lscr.io/linuxserver/plex` 的話，把 `image` 換成 `ghcr.io/plexassistant/plexassistant:latest` 即可，其他設定與 `/config` 都能沿用。要固定版本時，改用 `v` 加上[變更日誌](CHANGELOG.zh-TW.md)中的版本號，例如 `:v0.4.5`。

## 片頭與片尾標記

兩種標記都在掃描時寫入：

- **片頭**：比對同一季各集開頭的音訊指紋，一季至少要有兩集 strm。
- **片尾**：依序嘗試黑場偵測、Plex 已下載的偵測模型與畫面邊緣密度。可在 `environment` 設定 `ASSISTANT_CREDITS`：`auto`（預設）、`heuristic`（不使用模型）或 `model`（只使用模型）。

## 常用指令

指令在容器內以 Plex 的使用者 `abc` 執行，例如：

```bash
docker compose exec -u abc plex assistant marker list
```

- `assistant intro --section <N>`：對指定媒體庫重新偵測片頭。
- `assistant marker list`：列出已記錄的標記。
- `assistant marker set --item <ID> --kind <intro|credits> --start <毫秒> --end <毫秒>`：手動設定標記，之後的掃描不會覆蓋。
- `assistant tracks restore --account <ID> --show <ID> --part <ID>`：把該檔案的音軌與字幕選擇還原成改寫前的狀態，並忘掉記住的選擇。

## 限制

- 沒有 Web 介面。
- 本機檔案與 strm 混用的媒體庫可以運作，但只有 strm 項目會由 PlexAssistant 分析。
- 少數客戶端直接播放（Direct Play）時，可能不經過伺服器的轉碼器。
- 寫入媒體庫前會檢查 Plex 的資料庫版本；遇到尚未驗證的版本時只讀取、不寫入。

## 授權

[GPL-3.0](LICENSE)
