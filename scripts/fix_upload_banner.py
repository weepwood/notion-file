from pathlib import Path

path = Path("src/App.tsx")
text = path.read_text(encoding="utf-8")
old = '''            <div className="diagnostic-metrics">
              <div><span>{progress.stageCode === "hashing" ? "本地处理速度" : "当前上传速度（估算）"}</span><strong>{formatRate(progress.currentSpeedBytesPerSecond)}</strong></div>
              <div><span>{progress.stageCode === "hashing" ? "平均处理速度" : "平均有效速度（估算）"}</span><strong>{formatRate(progress.averageSpeedBytesPerSecond)}</strong></div>
              <div><span>当前阶段耗时</span><strong>{formatDuration(progress.stageElapsedMs)}</strong></div>
              <div><span>总耗时</span><strong>{formatDuration(progress.elapsedMs)}</strong></div>
              {progress.currentPart && progress.totalParts && <div><span>API 分片</span><strong>{progress.currentPart} / {progress.totalParts}</strong></div>}
            </div>
            {progress.endpointUrl && <div className="endpoint-row"><span>上传网址</span><code title={progress.endpointUrl}>{progress.endpointUrl}</code></div>}
            {progress.diagnosticHint && <div className="diagnostic-hint"><AlertCircle size={15} /><span>{progress.diagnosticHint}</span></div>}'''
new = '''            {progress.direction === "upload" && <>
              <div className="diagnostic-metrics">
                <div><span>{progress.stageCode === "hashing" ? "本地处理速度" : "当前上传速度（估算）"}</span><strong>{formatRate(progress.currentSpeedBytesPerSecond)}</strong></div>
                <div><span>{progress.stageCode === "hashing" ? "平均处理速度" : "平均有效速度（估算）"}</span><strong>{formatRate(progress.averageSpeedBytesPerSecond)}</strong></div>
                <div><span>当前阶段耗时</span><strong>{formatDuration(progress.stageElapsedMs)}</strong></div>
                <div><span>总耗时</span><strong>{formatDuration(progress.elapsedMs)}</strong></div>
                {progress.currentPart && progress.totalParts && <div><span>API 分片</span><strong>{progress.currentPart} / {progress.totalParts}</strong></div>}
              </div>
              {progress.endpointUrl && <div className="endpoint-row"><span>上传网址</span><code title={progress.endpointUrl}>{progress.endpointUrl}</code></div>}
              {progress.diagnosticHint && <div className="diagnostic-hint"><AlertCircle size={15} /><span>{progress.diagnosticHint}</span></div>}
            </>}'''
if old not in text:
    raise SystemExit("diagnostic banner snippet not found")
path.write_text(text.replace(old, new, 1), encoding="utf-8")
