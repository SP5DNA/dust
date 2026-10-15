# Proposal: adaptive group size

Status: proposal, nothing implemented yet. This note is for deciding before
any code is written.

## What it does

Today a group's length is drawn uniformly from the Group size range
(`rng.usize_in(min, max)` in `crates/cw-core/src/sampling.rs`). The proposal
adds an opt-in mode where the trainer learns how long a group you can copy
whole, and draws lengths near the edge of that:

- Copy groups whole and they get longer. Drop characters and they get
  shorter.
- Lengths always stay inside the Group size range you set. The range becomes
  the bounds the trainer works within.
- It is off by default. With it off, nothing changes, down to the random
  draws, so existing behaviour and tests are untouched.

## The model

The skill being measured is your **copy span**: how many characters you can
hold and write down from one hearing. It is modelled the way adaptive
psychophysics models a threshold (QUEST, Watson & Pelli 1983; the Ψ method,
Kontsevich & Tyler 1999), with group length as the stimulus.

**Observation.** After each group, record its length $L$ and whether it was
copied whole, $y \in \{0, 1\}$. This is the same test `GroupResult.correct`
already uses: the answer equals what was sent.

**Psychometric curve.**

$$
P(y = 1 \mid L, S, w) = (1 - \lambda)\,\sigma\!\left(\frac{S - L}{w}\right)
$$

- $S$, the **span**: the length you copy whole half the time.
- $w$, the **spread**: how sharply success falls off past the span.
- $\lambda = 0.03$, a fixed **lapse** rate. One slip on a short group does not
  collapse the estimate.
- $\sigma$ is the logistic function.

**Posterior on a grid.** $(S, w)$ lives on a fixed grid: $S \in [0.5, 20.5]$
in steps of 0.1, and $w \in \{0.4, 0.7, 1.0, 1.5, 2.5\}$, about 1,000 cells.
The posterior is kept exactly as log weights, so there is no approximation
and nothing to diverge:

$$
\log p(S, w \mid \text{data}) = \log p(S, w) + \sum_i \omega_i \left[y_i \log P_i + (1 - y_i)\log(1 - P_i)\right]
$$

- **Prior:** $S \sim \mathcal N(\tfrac{\min + \max}{2}, 3^2)$ over the grid, and
  $w$ uniform. The range you chose is a fair first guess at your span.
- **History weights $\omega_i$:** groups from the current session count 1.
  A session $k$ sessions back counts $0.5^{k/10}$, a half-life of 10 sessions,
  over at most the last 40 sessions. The estimate then follows you as you
  improve instead of being anchored by your first week.
- **Which sessions count:** only sessions in the same character-set mode.
  Callsign mode is excluded, because a callsign's length comes from its
  structure, not a draw.

**Choosing the next length: Thompson sampling plus a target.**

1. Draw one cell $(S^*, w^*)$ from the posterior.
2. For each allowed length $L \in [\min, \max]$, compute
   $p^*(L) = P(y = 1 \mid L, S^*, w^*)$.
3. Weight each length by how close it comes to the target success rate $t$:
   $$q(L) \propto \frac{\varepsilon}{n} + (1 - \varepsilon)\,\frac{e^{-(p^*(L) - t)^2 / 2\tau^2}}{Z}$$
   with $n$ the number of allowed lengths, variety $\varepsilon = 0.1$ and
   width $\tau = 0.15$.
4. Sample $L \sim q$.

Step 1 explores while the posterior is wide and settles as it narrows.
Step 3 keeps a spread of lengths around the target instead of one fixed
length, and the $\varepsilon$ floor means every length in your range still
turns up now and then.

**Target presets.** These are shown as "Aim for":

| Preset | Groups copied whole | Feels like |
| --- | --- | --- |
| Comfortable | 85% | Mostly clean copy |
| Balanced (default) | 70% | About a third of groups trip you up |
| Stretch | 50% | Right at your span |

For reference, Wilson et al. 2019 (*The Eighty Five Percent Rule for
optimal learning*) put the best training accuracy near 85%. Counting a whole
group as one item, that is Comfortable. Balanced asks for a bit more: 70% of
five-character groups copied whole is still about 93% of characters.

**Cost.** One update is about 1,000 logistic evaluations. Seeding from history
groups observations by (session, length, outcome) first, so it costs one
update per distinct length per session, not one per group. Both are well
under a millisecond, on wasm too.

## Simulation

I checked the policy with a Python prototype (not committed) against
synthetic learners, with Group size 2–10 and an 80% target:

| Learner | After 20 groups | After 60 | After 200 | Whole-group rate, last 20 |
| --- | --- | --- | --- | --- |
| span 4, spread 0.7 | 5.0 ± 1.0 | 4.2 ± 0.5 | 4.0 ± 0.3 | 75–85% |
| span 6, spread 0.8 | 6.2 ± 1.1 | 6.0 ± 0.7 | 5.9 ± 0.3 | 75–90% |
| span 9, spread 1.5 | 8.8 ± 1.2 | 8.5 ± 0.9 | 8.7 ± 0.6 | 80–95% |
| span 4 → 7, improving | 4.8 ± 1.2 | 4.5 ± 0.8 | 6.8 ± 1.0 | 75–90% |

It settles within a couple of sessions and tracks a learner who improves. The
whole-group rate stays near the target throughout.

## Where it goes in the code

**cw-core**

- New `span.rs` holding `SpanBelief`:
  - `from_history(history, settings)`
  - `learn(len, whole)`
  - `draw_length(min, max, target, rng)`
  - `estimate() -> (mean, sd)`
  - `forecast(min, max, target) -> Vec<LengthForecast>`, giving each
    length's chance of being drawn next and its predicted whole-copy rate.
    It averages over the whole posterior, which is what the UI shows.
- `sampling.rs`: `Sampler` holds a `SpanBelief` beside its `CharSamplingState`.
  - `generate_training_group` asks it for the length only when adaptive is on,
    the range is not fixed, and the mode is not callsigns. Otherwise it keeps
    the existing `rng.usize_in` call, so the draw sequence is identical when
    the feature is off.
  - `Sampler::learn` already receives each group's `CopyTally`. The length and
    whether it was copied whole are read off that, so `GroupSource` does not
    change.
- `settings/mod.rs`: two fields in `CurriculumSettings`, in the Session shape
  section so its reset button covers them:
  - `adaptive_group_size: bool` (default `false`)
  - `group_size_target: GroupSizeTarget` (`Comfortable | Balanced | Stretch`,
    default `Balanced`)

  Both are `serde(default)`, so older saves load with the feature off.
- `stats.rs`:
  - `group_length_forecast(settings, sessions)` for the Sampling tab.
  - `span_history(settings, sessions)`: the span estimated after each session,
    found by replaying the history, for the Overview chart.

Nothing new is stored. The belief is rebuilt from the session history each
time, the same way the character sampler's is, and the history already holds
every group's sent and received text. No migration is needed.

**App and UI**

Settings holds only the controls. Everything the model has learned is shown
on the Stats screen, so the settings page doesn't grow.

- **Settings → Session shape, under Group size:**
  - A switch, "Adapt group size to me". Its description ends with "Your copy
    span is under Stats."
  - When the switch is on, an "Aim for" segmented control (Comfortable ·
    Balanced · Stretch) with a one-line explanation. Nothing else.
  - With a fixed group size, the switch is disabled and says why. In callsign
    mode it is hidden along with the range.
- **Stats → Overview:** a "Copy span" card under "Accuracy over time":
  - The headline span ("5.9 ± 0.2 chars").
  - A line of the span estimated after each session, in the same style as the
    accuracy chart, with the change over the period shown.
  - It is shown whenever there is history, adaptive or not. It is a measure of
    progress in its own right, and it's how people will find the feature.
- **Stats → Sampling:** a "Group length" card next to the character snapshot.
  Each length in the range gets a row with its chance of being drawn next and
  how often you copy a group that long whole. With adaptive off, the chances
  are even, and the card says so.
- **Results screen** (optional): one line, e.g. "Groups ran 4–6 chars · span
  5.6 → 5.9".
- `assets/styles.css`: nothing new is expected. The cards reuse the existing
  chart, bar and KPI styles.

## How it interacts with auto level

Auto level moves your level on per-character session accuracy. Adaptive
length holds whole-group success near the target, which tends to keep
per-character accuracy high. That can make level-ups come a little sooner. A
new character then lowers whole-group success, and groups shorten until it
settles. That is reasonable behaviour, but it is two feedback loops on
different timescales.

- **Proposal for v1:** leave auto level as it is and watch.
- **Alternative:** with adaptive on, count a session toward a level-up only if
  its mean group length was at least the range minimum + 1.

## Tests

- **Core:**
  - Successes at long lengths raise the span; failures lower it.
  - Draws never leave `[min, max]`.
  - A fixed range returns that size.
  - With the feature off, the draw sequence is byte-identical to today's.
  - Older saves load with it off.
  - Reset puts both fields back.
  - A simulated learner of span 6 is estimated within ±0.5 after 200 groups.
  - The existing soak test is extended to cover adaptive on.
- **UI:**
  - The switch exists and is off by default.
  - It is disabled for a fixed size and hidden in callsign mode.
  - The target segments appear only while the switch is on, and they write the
    setting.
  - The Overview span card and the Sampling length card render from history,
    with adaptive both on and off.

## Decisions for you

1. Are the target presets right (85 / 70 / 50% of groups copied whole), and is
   Balanced the right default?
2. Should the span be learned separately per speed? v1 learns one span with
   recency weighting. Per-speed spans would need more data before they're
   trustworthy.
3. Auto level: leave it as is (proposed), or add the guard above?
4. Should the signal be whole-group copied, or per-character within the group?
   Whole-group is proposed: it's what span means, and it already exists in the
   history. Per-character would separate "didn't know the letter" from "lost
   the group", at the cost of a second model.
