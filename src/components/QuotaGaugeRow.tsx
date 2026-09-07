// 利用枠ゲージ 1 本 (FR-C-80〜95)。
//
// ★ 外部チャートライブラリを使わない自前描画 (ADR-0005 / FR-C-163)。
// ★ 出所ラベル (実値 / 推定 / 取得不可) を**必ず**出す (FR-C-81 / NFR-40)。
// ★ 超過を 100% でクランプしない (FR-C-90)。

import type { QuotaGauge } from '../types/dto'
import { gaugeText, isStale, severity, sourceLabel, sourceNote } from '../lib/format'

export function QuotaGaugeRow({ gauge, now }: { gauge: QuotaGauge; now: number }) {
  const sev = severity(gauge.used_pct)
  const stale = isStale(gauge.origin, now)
  const note = sourceNote(gauge.origin)

  // バーは 100% までを描き、超過分は別のセグメントとして描く。
  // 「上限に達した」ように見せないため、率そのものはクランプしない。
  const pct = gauge.used_pct ?? 0
  const filled = Math.min(pct, 100)
  const over = Math.max(0, Math.min(pct - 100, 100))

  return (
    <div className="gauge">
      <div className="gauge-head">
        <span className="gauge-label">{gauge.label}</span>
        <span className={`chip source-${gauge.origin.source}`}>{sourceLabel(gauge.origin)}</span>
        {stale && (
          <span className="chip warn" title="観測から 15 分以上経過しています">
            鮮度注意
          </span>
        )}
      </div>

      <div className="gauge-track" role="img" aria-label={gaugeText(gauge)}>
        {gauge.unlimited ? (
          <div className="gauge-unlimited" />
        ) : (
          <>
            {/* transform だけを使う (GPU 合成。FR-C-163 / NFR-05) */}
            <div
              className={`gauge-fill ${sev}`}
              style={{ transform: `scaleX(${filled / 100})` }}
            />
            {over > 0 && (
              <div className="gauge-over" style={{ transform: `scaleX(${over / 100})` }} />
            )}
          </>
        )}
      </div>

      <div className="gauge-foot">
        <span className="gauge-text">{gaugeText(gauge)}</span>
        {/* FR-C-90: 超過額は別建てで明示する */}
        {gauge.overage !== null && gauge.overage > 0 && (
          <span className="gauge-overage">超過 {gauge.overage.toFixed(2)}</span>
        )}
      </div>

      {/* FR-C-82 / 83: 推定の但し書きと、取得不可の対処 */}
      {note && <p className="fineprint">{note}</p>}
    </div>
  )
}
