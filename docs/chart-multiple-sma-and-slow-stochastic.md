# Multi-SMA and multiple Stochastic %K/%D charts

Terminal-rs keeps the existing chart layout and GPU drawing pipeline, but
supports multiple independent period settings on one chart.

## Drag to resize indicator panels

The price chart and each oscillator (Stochastic, ADX, RSI, MACD and others)
are separated by a thin horizontal resize line with a central grip.

- **Drag the first divider** (between the price chart and the first
  oscillator) **down** to shrink all indicator panels together and give
  the price chart more room. Drag **up** to expand the entire indicator
  stack while preserving each panel's relative size.
- **Drag a divider between two oscillators** to change only the heights
  of those neighboring panels. Their combined height stays the same.
- **Double-click any indicator divider** to reset all indicator heights
  on that chart to the default proportions.

The cursor changes to vertical resize when hovering over a divider.
New charts default to roughly **12% of the plot height per oscillator**
(up to 60% in total), leaving more room for price candles than before.
Drag limits keep both the main price chart and individual oscillators
visible even in compact windows.

Custom heights are stored in the active chart's saved layout and restored
after restarting the app. Resizing the application window preserves the
relative height distribution. Existing layouts without the new fields
load with the updated defaults. Chart panning, scrolling, indicator
calculations, and the independent SMA/Stochastic parameters are untouched.

## Moving averages

Right-click a chart → **Indicators** → **Multiple SMA lines…** (or
**Moving Averages** → **Simple Moving Average · add lines**).
Select any of **5, 20, 60, 120**, or enter a custom length (1–1000).
The preset no longer includes SMA 10, but it can still be entered manually.
Adding an SMA does not remove another SMA.

The **On this chart** rows show each SMA separately. Drag or double-click
the length to edit its numeric value; **✕** removes only that line.
An attempt to edit a line to exactly match an existing one merges the
duplicates. The old `SMA 20` or `SMA 120` terminal commands still work.

An SMA of length *N* averages the last N **bars**, not always N calendar
days: on a **1D** chart these are N trading sessions (5/20/60/120
trading-day moving averages), while on a 5-minute chart these are 5-minute
bars.

## Stochastic %K and %D (5,2,2 / 10,5,5 / 20,10,10)

Right-click → **Indicators** → **Stochastic %K + %D studies…** (or
**Momentum** → **Stochastic %K + %D · add studies**). Add each provided
combination or enter three custom integer parameters (1–1000):

- **5,2,2**: raw %K length 5, SMA smoothing of raw %K over 2 bars,
  SMA smoothing of slow %K over 2 bars → plot *both %K and %D*.
- **10,5,5**: the same, with 10, 5 and 5 periods.
- **20,10,10**: the same, with 20, 10 and 10 periods.

The study's code is `STOCH`; each instance has **two lines: red %K and
purple %D** (colors are tuned for light and dark themes). Each combination
is drawn on its **own 0–100 oscillator panel** below the main chart, with
reference levels 20/80. Each period triple can be edited independently
under **On this chart**, or removed with **✕**. Further combinations may
be added (up to the existing six-oscillator-pane visible limit).

The older `STOCHD` indicator (slow %D only, optionally grouped in one
pane) remains available for **previously saved layouts**. New charts use
the two-line `STOCH` study instead.

The existing stochastic calculation is unchanged:

```text
Raw %K(n) = 100 × (Close - LowestLow(n)) / (HighestHigh(n) - LowestLow(n))
Slow %K   = SMA(Raw %K, smoothK)
Slow %D   = SMA(Slow %K, smoothD)
```

For a flat high–low range, the existing stochastic engine uses its
defined neutral value (50). Historical values without sufficient
lookback are excluded from the plotted line, not fabricated.

## One-click preset and saved layouts

Use **Indicators → Apply Korean preset · SMA 5/20/60/120 + Stoch %K/%D ×3**
to replace the SMA and any STOCH/STOCHD studies on the **current chart**
with the requested four moving averages and three K/D oscillator panels
in one click. Other indicators remain untouched.
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
SMA 20
SMA 60
SMA 120
STOCH 5 2 2
STOCH 10 5 5
STOCH 20 10 10
```

Other indicators follow the existing palette. No changes to foreign-market
candles or data providers are needed. This is a read-only analysis feature,
not a trading or order-entry capability.
