# Multi-SMA and Slow Stochastic %D charts

Terminal-rs keeps the existing chart layout and GPU drawing pipeline, but
supports multiple independent period settings on one chart.

## Moving averages

Right-click a chart → **Indicators** → **Multiple SMA lines…** (or
**Moving Averages** → **Simple Moving Average · add lines**).
Select any of **5, 10, 20, 60, 120**, or enter a custom length (1–1000).
Adding an SMA does not remove another SMA.

The **On this chart** rows show each SMA separately. Drag or double-click
the length to edit its numeric value; **✕** removes only that line.
An attempt to edit a line to exactly match an existing one merges the
duplicates. The old `SMA 20` or `SMA 120` terminal commands still work.

An SMA of length *N* averages the last N **bars**, not always N calendar
days: on a **1D** chart these are N trading sessions (5/10/20/60/120
trading-day moving averages), while on a 5-minute chart these are 5-minute
bars.

## Slow Stochastic %D (5,3,3 / 10,6,6 / 20,12,12)

Right-click → **Indicators** → **Slow Stochastic %D lines…** (or
**Momentum** → **Slow Stochastic %D · add lines**). Add each provided
combination or enter three custom integer parameters (1–1000):

- **5,3,3**: raw %K length 5, SMA smoothing of raw %K over 3 bars,
  SMA smoothing of slow %K over 3 bars → plot *slow %D only*.
- **10,6,6**: the same, with 10, 6 and 6 periods.
- **20,12,12**: the same, with 20, 12 and 12 periods.

The new study's code is `STOCHD`, and each independent instance is one
colored line. All STOCHD instances are plotted **together in a single
0–100 oscillator panel**, with separate legend values and reference levels
20/80. Each period triple can be edited individually under
**On this chart**, or removed with **✕**.

The existing `STOCH` indicator (original %K **and** %D) is intentionally
unchanged, and still loads from older layouts.

The formula for the new study is:

```text
Raw %K(n) = 100 × (Close - LowestLow(n)) / (HighestHigh(n) - LowestLow(n))
Slow %K   = SMA(Raw %K, smoothK)
Slow %D   = SMA(Slow %K, smoothD)
```

For a flat high–low range, the existing stochastic engine uses its
defined neutral value (50). Historical values without sufficient
lookback are excluded from the plotted line, not fabricated.

## One-click preset and saved layouts

Use **Indicators → Apply Korean preset · SMA 5/10/20/60/120 + Slow %D ×3**
to replace the SMA and STOCHD studies on the **current chart** with all
eight requested series in one click. Other indicators remain untouched.
Newly opened Korean stock/index charts start with this preset.

Existing saved chart layouts are **not automatically overwritten**; apply
the preset to those once if desired. Multiple instances and edited
periods use the existing saved `indicators` array in the layout file,
so no migration is needed. On reopening the saved layout, settings return
with the same periods.

The existing command entry also supports adding/toggling individual
instances:

```text
SMA 5
SMA 10
SMA 20
SMA 60
SMA 120
STOCHD 5 3 3
STOCHD 10 6 6
STOCHD 20 12 12
```

Preset colors follow the existing palette, with different colors for
independent instances; no changes to foreign-market candles or data
providers are needed. This is a read-only analysis feature, not a
trading or order-entry capability.
