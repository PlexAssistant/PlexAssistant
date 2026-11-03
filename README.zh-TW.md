# PlexAssistant

[English](README.md) | 正體中文

讓 Plex 原生支援 STRM:`.strm` 項目顯示真實媒體資訊、可以播放,
而且不會把雲端硬碟帳號打出風控。

## 原理

Plex 把 `.strm` 當成 35 位元組的純文字:介面裡沒有時長、解析度、
音軌;每次播放還用任意的 User-Agent 重新解析檔案裡的 URL。這種
流量模式正是雲端硬碟帳號被限速的原因。

PlexAssistant 包裝 Plex 在媒體庫工作上會啟動的兩支程式:

- **Scanner**:Plex 要求分析時,解析 strm 目標,用 ffprobe 探測
  直鏈(與解析時同一個 User-Agent;115 這類 CDN 會把連結綁定給
  它),把時長、編碼、解析度、HDR 徽章、字幕語言直接寫進媒體庫,
  欄位形狀與官方掃描器一致。打開詳情頁也會補上還沒有媒體資訊的
  strm:Plex 認為已經分析過的項目,只靠掃描會一直空著。
- **Transcoder**:播放時把 `-i` 的 strm 路徑換成快取直鏈,再交給
  原始轉碼器。連結中途失效,毫秒內刷新、重試一次。

兩者之間的常駐 daemon 維護持久連結快取:每個目標只解析一次,
之後持續信任,失效才處理。掃描輪內發現死鏈,每個目標只刷新
一次;持續失效的目標退避 1/2/4/7 天,不再重複探測。掃描側的
解析受每網域熔斷器與每小時配額約束;播放永遠放行。

## 使用

把現有 Plex 部署裡的映像換掉即可,其他不用改:

```yaml
services:
  plex:
    image: ghcr.io/plexassistant/plexassistant:latest
```

```bash
docker compose up -d
```

之後在 Plex 把媒體庫指向 strm 資料夾,媒體資訊自動出現。

## 限制

- 沒有 Web 介面。
- 本地檔案與 strm 混用的媒體庫可以運作,但只有 strm 項目獲得分析。
- 少數直接播放(direct play)的客戶端可能繞過伺服器端轉碼。
- 片頭標記需執行 `assistant intro --section N`。片尾標記在掃描時
  寫入:黑窗,然後是 Plex 已經存放的模型,最後是邊緣密度。
  `ASSISTANT_CREDITS` 可選 `auto`(預設)、`heuristic` 或 `model`。
