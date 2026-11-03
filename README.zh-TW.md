# PlexAssistant

[English](README.md) | 正體中文

讓 Plex 原生支援 `.strm`：顯示媒體資訊、可以播放，也不會因為反覆解析連結讓雲端硬碟帳號被限速。

## 運作方式

PlexAssistant 取代 Plex 的掃描器與轉碼器：

- **掃描**：以 ffprobe 探測 strm 的直鏈，把時長、編碼、解析度與字幕語言寫進媒體庫，並偵測片頭與片尾。
- **播放**：把 strm 換成快取的直鏈，再交給原本的轉碼器；連結失效時自動刷新。
- **常駐服務**：每個連結只解析一次並持續快取。掃描觸發的解析有熔斷與配額限制，播放不受限。

## 快速開始

1. 下載 [`compose.yaml`](compose.yaml)，依註解修改 strm 路徑與 `PUID`、`PGID`、`TZ`。
2. 執行 `docker compose up -d`。
3. 在 `http://<主機>:32400` 新增指向 `/media` 的媒體庫。

已在使用 `linuxserver/plex` 的話，把 `image` 換成 `ghcr.io/plexassistant/plexassistant:latest` 即可。

## 限制

- 只分析 strm 項目，本機檔案維持 Plex 原本的處理方式。
- 部分直接播放（Direct Play）的客戶端可能繞過轉碼器。
- 片頭偵測需要同一季至少兩集 strm。
- 遇到尚未驗證的 Plex 資料庫版本時，只讀取、不寫入。

## 授權

[GPL-3.0](LICENSE)
