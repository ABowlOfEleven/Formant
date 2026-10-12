# Formant built-in effects: reference designs and IP survey

Scope: 48 kHz, mono, per-sample, CPU-only, low latency (live self-monitoring).

Clean-room rule for this project: implement only from expired patents, published papers, standards, and publicly documented hardware behavior (manuals, spec sheets, measurements published by others). Never from decompiled or disassembled commercial code. Open-source code under a GPLv3-compatible license may be read, but cite it.

Patent status note. Status values below come from Google Patents "legal status" fields and from the rule that a US utility patent filed after 8 June 1995 runs 20 years from its earliest non-provisional filing date (plus any term adjustment). Google Patents labels its status as an assumption, not a legal conclusion. This document is engineering due diligence, not legal advice. Rule of thumb: any US patent with an earliest filing date before about 2004 is expired now (October 2026) even with typical term adjustment. Anything filed 2006 or later can still be active.

---

## 0. Active-patent watch list (summary)

Avoid copying the claimed combination of these. In most cases a classic, published design gives the same user-facing result.

| Patent | Assignee | Priority | Status (Google) | What to avoid |
|---|---|---|---|---|
| US 8,488,811 B2 "Audio-peak limiting in slow and fast stages" | Dolby Laboratories | 2006-08-09 | Active, est. exp. 2030-04-24 | A rate-limited slow gain stage, then a delay line, then a second fast gain stage that removes the residual peaks found in the delay line. Use a single-stage lookahead limiter (section 2). |
| US 11,496,099 B2 "Systems and methods for limiter functions" | Mimi Hearing Technologies | 2020-07-28 | Active, est. exp. 2041-05-12 | Block-based gain contour computed by a backward pass over current and next block, then a forward pass with "instant gain ascent and smooth gain decay". Use a per-sample sliding-window minimum plus smoothing filter (section 2), which is prior art (Zolzer DAFX 2002 and others). |
| US 11,322,170 B2 (and continuation US 12,051,435) "Audio de-esser independent of absolute signal level" | Dolby International / Dolby Labs | 2017-10-02 | Active, est. exp. 2039-05-12 | Frequency-domain buffers, sibilance energy relative to the previous buffer's energy envelope, plus a zero-crossing-rate test, to gate de-esser gain. Do not use ZCR plus relative-energy detection in an FFT de-esser. |
| US 11,817,115 B2 "Enhanced de-esser for in-car communication systems" | Cerence (ex Nuance) | 2016-05-11 | Active, est. exp. 2038-10-11 | Spectral-envelope analysis with psychoacoustic "sharpness" driving de-esser weights, with soft and hard sharpness thresholds. Do not use a sharpness metric as the de-esser detector. |
| US 8,345,887 B1 "Computationally efficient synthetic reverberation" | Sony Interactive Entertainment | 2007-02-23 | Active, est. exp. 2030-08-31 | Claims an FDN (delay lines, matrix transform, feedback) where a subset of the outputs forms a multi-channel reverb signal. The FDN itself is old prior art (Stautner and Puckette 1982, Jot 1992). Keep to the published Jot/Stautner forms with mono output; do not copy this patent's specific output-subset and matrix choices. |
| US 8,315,398 B2 "System for adjusting perceived loudness of audio signals" | DTS Inc. | 2007-12-21 | Active, est. exp. 2030-11-01 | Loudness-model filters driving a gain that holds perceived loudness constant over time. A BS.1770 K-weighted meter with a slow, gated make-up gain is standard practice, but read the claims before shipping an "auto-loudness" AGC. See section 10. |
| US 2023/0215450 A1 "Automatic noise gating" | Inventor Ryan Meng-Wei Lu (assignee not shown) | about 2022 | Application, status unknown | VAD finds non-speech segments, level detector measures their noise, gate threshold is set automatically from that level. Do not ship a "VAD-measured noise floor sets gate threshold" feature without checking status. |
| US 11,302,347 / US 11,688,418 "Low latency automixer integrated with voice and noise activity detection" | (automixer vendor; verify) | about 2019 | Active | VAD output overriding automixer channel gating. Not directly relevant to a single-channel gate, but relevant if Formant adds multi-mic auto-mixing. |
| US 10,230,346, US 9,002,030 (VAD) | various | 2010s | Active | Specific VAD algorithms used to mute a mic. Use an energy-plus-hysteresis gate, or a published open VAD (WebRTC VAD BSD, RNNoise BSD) only after checking its own patent notes. |

Confirmed expired and safe to study: Blackmer RMS detector (US 3,681,618), Jot and Chaigne FDN (US 5,491,754), Creative/Jot environmental reverb (US 6,188,769), Orban limiter and clipper family (US 4,208,548, US 4,249,042, US 4,412,100, US 4,460,871, US 5,737,434), AKG distortion-cancelled clipper (US 5,168,526), Waller program-dependent release (US 7,532,730, expired 2022), Amazon low-latency limiter (US 9,661,438, expired for fee non-payment), Lexicon/Griesinger 1980s and early 1990s patents (any 1992 grant is past its term), adaptive-filter de-esser (US 6,373,953).

No Waves L1 limiter patent was found in a search. The L1 is documented publicly only as a lookahead peak limiter with IDR dither (Gerzon and Craven noise shaping). Do not assume any particular internal design.

---

## 1. Compressor

### References

- D. Giannoulis, M. Massberg, J. D. Reiss, "Digital Dynamic Range Compressor Design: A Tutorial and Analysis", JAES 60(6), 399 to 408, 2012. Author copy: https://www.eecs.qmul.ac.uk/~josh/documents/2012/GiannoulisMassbergReiss-dynamicrangecompression-JAES2012.pdf
- D. E. Blackmer, "RMS circuits with bipolar logarithmic converter", US 3,681,618, granted 1972, assigned to dbx Inc. Expired. https://patents.google.com/patent/US3681618 . Log-domain RMS detection, about 6 mV/dB. Related dbx expired patents: US 4,234,804 (signal correction for gain control), US 4,331,931 (gain control systems), US 4,065,682 (log converter, cites Blackmer).
- J. K. Waller Jr., "Audio dynamics processing control system", US 7,532,730, priority 2000-04-28. Expired 2022. Release time constant changed when a differential time-constant control signal rises (program-dependent release). https://patents.google.com/patent/US7532730B2/en . Also Waller, US 5,402,498 "Automatic intelligent audio-tracking response circuit" (1990s, expired).
- U. Zolzer (ed.), DAFX: Digital Audio Effects, 2nd ed., Wiley 2011, chapter 4 (nonlinear processing, dynamics). Standard textbook design for feedforward compressor, limiter, gate, with lookahead.
- F. Ramos, "Digital Compressor Design", or the Reiss and McPherson book "Audio Effects: Theory, Implementation and Application", CRC 2014, ch. 6.
- Universal Audio public manuals and webzine articles for 1176LN and LA-2A behavior (e.g. "LA-2A history" by Will Shanks, UA WebZine 2003). Teletronix original manual quoted there.
- SSL G-series bus compressor public spec sheets. Fairchild 670 public spec sheets (reissue manual).

### Key algorithm details

Work in the log domain (dB) for the gain computer and, for best behavior, for smoothing too. All formulas below follow Giannoulis et al.

Level detection, input to dB:

    x_G = 20 log10(|x| + eps)          (peak, per sample)
    x_G = 10 log10(ms + eps)           (RMS, where ms is a one-pole mean square)

RMS (Blackmer style) mean square, one pole:

    ms[n] = a_rms ms[n-1] + (1 - a_rms) x[n]^2,   a_rms = exp(-1 / (tau_rms fs))

tau_rms of 5 to 50 ms for vocals. RMS detection gives a smoother, "musical" compressor (dbx 160 character). Peak detection is needed for limiters and 1176-style fast catches.

Static curve (gain computer) with soft knee, threshold T, ratio R, knee width W (all dB):

    if 2 (x_G - T) < -W:        y_G = x_G
    if 2 |x_G - T| <= W:        y_G = x_G + (1/R - 1) (x_G - T + W/2)^2 / (2W)
    if 2 (x_G - T) > W:         y_G = T + (x_G - T) / R

    gain computer output (dB):  x_L = x_G - y_G       (always >= 0, amount of reduction)

Time constant to coefficient. Giannoulis uses tau as the time to reach 1 - 1/e (63 percent):

    alpha = exp(-1 / (tau fs))

Many hardware specs define attack as the 10 to 90 percent time. To match a spec in that form, use alpha = exp(-ln(9) / (t_10_90 fs)) (that is, tau = t / 2.197). Pick one and document it in the UI tooltip.

Smoothing (peak detector) variants, applied in the gain domain on x_L (this is the paper's recommended "log domain, after the gain computer" placement):

1. Smooth branching (recommended general purpose):

        if x_L[n] > y_L[n-1]:  y_L[n] = aA y_L[n-1] + (1 - aA) x_L[n]
        else:                  y_L[n] = aR y_L[n-1] + (1 - aR) x_L[n]

2. Smooth decoupled (release then attack in series; release envelope shapes the attack):

        y1[n]  = max(x_L[n], aR y1[n-1] + (1 - aR) x_L[n])
        y_L[n] = aA y_L[n-1] + (1 - aA) y1[n]

   Decoupled gives a slower apparent release and a longer apparent attack than set, and it does not overshoot. It behaves more like many analog units.

3. Non-smooth versions (release toward zero instead of toward the input) produce discontinuities in the derivative; avoid.

Paper's findings to follow: smooth branching in the log domain, placed after the gain computer, gives the most predictable attack and release, with no "attack lag" at the knee. Placing the detector before the gain computer (on the level) changes the effective knee and makes release depend on level.

Gain, make-up and mix:

    c_dB[n] = M - y_L[n]                 (M is make-up gain in dB)
    g[n]    = 10^(c_dB[n] / 20)
    out     = mix * g[n] x[n] + (1 - mix) x[n]     (parallel compression; dry and wet are sample-aligned)

Auto make-up: M = -(T - T/R) / 2 is the common approximation (half of the static reduction at 0 dBFS). Expose it as a toggle, not a hidden default.

Feedforward vs feedback:

- Feedforward: detector reads the input. Exact ratio, can do any ratio up to infinity, can use lookahead. Used by SSL G bus (VCA, feedforward), dbx 160, most plugins.
- Feedback: detector reads the output. Ratio is self-limiting (cannot reach infinity:1), and attack and release interact with gain reduction, which gives a softer, "program dependent" feel. Used by 1176 (FET), LA-2A (opto, sidechain from output), Fairchild 670 (variable-mu). Digital feedback needs a one-sample delay in the loop; at 48 kHz this is harmless. To emulate, compute y_G from the previous output sample level, or use a feedforward detector with a ratio mapping R_ff = R_fb chosen so the static curve matches; the dynamic feel comes mostly from release behavior.

Program-dependent release models (all public domain math):

1. Dual time constant, parallel max ("auto release", SSL and dbx style):

        y_fast = smooth_branching(x_L, attack, R_fast = 50 to 150 ms)
        y_slow = smooth_branching(x_L, attack, R_slow = 1 to 3 s)
        y_L    = max(y_fast, y_slow)

   Short transients recover at the fast rate. Sustained compression charges y_slow, so release is slow after long loud passages.

2. Cascaded (y_slow fed from y_fast instead of x_L). Similar result, smoother.

3. Duration-dependent release (Waller US 7,532,730, expired): measure how long the gain has been reduced (or how fast x_L is rising) and interpolate tau_R between a short and a long value.

4. Opto (LA-2A T4B) memory model: a "light history" state h integrates x_L with a slow time constant (seconds to tens of seconds). Release coefficient is two-stage: a fast part (about 60 ms to 50 percent recovery) and a slow part whose time constant scales with h (0.5 to 5 s typical, 1 to 15 s per the original manual). Attack is fixed at about 10 ms and slightly level dependent. The ratio is soft (about 3:1 in Compress mode, higher in Limit mode), and the knee is very wide because the photocell response is nonlinear.

Sidechain filter: a 2nd-order high-pass (RBJ HPF, Q 0.707) on the detector input only, 20 to 300 Hz. This stops plosives and low-frequency rumble from pumping the vocal.

Character / saturation: apply a mild waveshaper (section 6) to the gain-reduced signal, with drive scaled by gain reduction for FET or variable-mu flavor. Keep it optional and default off.

### Classic-hardware behavior worth emulating (public specs)

- UREI / UA 1176LN (FET, feedback-style, peak): attack 20 us to 800 us; release 50 ms to 1.1 s; ratios 4, 8, 12, 20; "all buttons in" (British mode) gives a very hard, distorted, high ratio with a changed bias, a pronounced knee and audible attack overshoot. Attack and release knobs are reversed (clockwise is faster). Release is program dependent. Threshold is fixed; the Input knob drives into it.
- Teletronix LA-2A (opto, T4B cell): attack about 10 ms average; release about 60 ms for 50 percent, then 0.5 to 5 s (UA spec) or 1 to 15 s (original manual) to full release, longer after sustained compression. Only two main controls: Peak Reduction and Gain, plus Compress/Limit switch. Sidechain has an "R37" high-frequency emphasis trim (more reduction on treble).
- SSL G-series bus compressor (VCA, feedforward): attack 0.1, 0.3, 1, 3, 10, 30 ms; release 0.1, 0.3, 0.6, 1.2 s and Auto (dual release); ratio 2, 4, 10. Hard knee.
- Fairchild 670 (variable-mu, feedback, very soft knee, ratio rises with level): six time constants. Pos 1: 0.2 ms / 0.3 s. Pos 2: 0.2 ms / 0.8 s. Pos 3: 0.4 ms / 2 s. Pos 4: 0.4 ms / 5 s. Pos 5: 0.4 ms / 2 s (single peaks) to 10 s (multiple peaks), automatic. Pos 6: 0.2 ms / 0.3 s single peaks, 10 s multiple peaks, 25 s for sustained high level, automatic.
- dbx 160 (VCA, Blackmer RMS, feedforward): "OverEasy" soft knee; attack and release are program dependent, set by the RMS detector; ratio 1:1 to infinity:1.

### Recommended control set

| Control | Range | Default (vocal) |
|---|---|---|
| Threshold | -60 to 0 dBFS | -18 dB |
| Ratio | 1:1 to 20:1, plus infinity (limit) | 3:1 |
| Knee | 0 to 24 dB | 6 dB |
| Attack | 0.05 to 100 ms | 5 ms |
| Release | 10 ms to 2.5 s | 120 ms |
| Auto release | off / on (dual time constant, 80 ms and 1.5 s) | off |
| Detector | Peak / RMS (RMS window 5 to 50 ms) | Peak |
| Topology | Feedforward / Feedback | Feedforward |
| Sidechain HPF | off, 20 to 300 Hz | 80 Hz |
| Make-up | -12 to +24 dB, plus Auto | 0 dB |
| Mix | 0 to 100 percent | 100 percent |
| Character | Clean / FET / Opto / Vari-mu presets that set curve, release model and saturation | Clean |
| Meters | input, output, gain reduction | |

Preset styles: "FET" = feedback, peak, ratios 4/8/12/20 plus "All", attack 0.02 to 0.8 ms, release 50 ms to 1.1 s. "Opto" = feedback, fixed 10 ms attack, two-stage memory release, one Peak Reduction knob. "Bus" = feedforward VCA with SSL step values.

### Pitfalls

- Gain smoothing in the linear domain gives level-dependent attack times. Smooth in dB.
- Do not compute log10 of zero; add eps (1e-20 for power) or clamp to -150 dB.
- Denormals in the release tail. Flush to zero (set FTZ/DAZ on x86) or add a tiny offset.
- Parallel mix must use the exact same latency for dry and wet. Lookahead in the compressor adds latency to both.
- Very fast attack (< 1 ms) on a peak detector modulates the waveform within a cycle and causes distortion of low notes. This is part of the 1176 sound, but it is a bug on a "clean" setting. Use a minimum of about 0.3 ms for "Clean", or add lookahead.
- Feedback topology with high ratio can oscillate if the detector has no smoothing; always keep at least a short attack.
- Auto make-up from the static curve over-compensates at low thresholds. Cap it.

---

## 2. Limiter and true-peak

### References

- ITU-R BS.1770-5 (11/2023), "Algorithms to measure audio programme loudness and true-peak audio level". Annex 2: true-peak measurement. https://www.itu.int/rec/R-REC-BS.1770
- EBU Tech 3341 (loudness metering, EBU mode) and EBU R 128. https://tech.ebu.ch/publications
- U. Zolzer, DAFX, 2nd ed., ch. 4: lookahead limiter with delay line and peak hold.
- Giannoulis et al. 2012 (above) for the detector and smoothing.
- Orban, US 4,208,548 "Apparatus and method for peak-limiting audio frequency signals" (Optimod-AM 9000A distortion-cancelled clipper). Expired.
- AKG (Orban design), US 5,168,526 "Distortion-cancellation circuit for audio peak limiting". Expired.
- Orban, US 5,737,434 "Multi-band audio compressor with look-ahead clipper", filed 1996. Expired.
- Orban, US 4,249,042 and US 4,460,871 "Multiband cross-coupled compressor with overshoot protection circuit". Expired.
- Amazon, US 9,661,438 "Low latency limiter", priority 2015, listed "Expired, fee related". Describes delay, peak-to-threshold gain, fast reduction, slow recovery; good evidence that the basic lookahead limiter is common knowledge.
- ACTIVE, avoid: Dolby US 8,488,811 (two-stage slow plus fast with delay) and Mimi US 11,496,099 (block backward/forward gain-contour pass). See section 0.

### Key algorithm details

Single-stage lookahead brickwall limiter (Zolzer-style, per sample, latency L samples):

1. Delay the audio by L samples (L = lookahead, 1 to 5 ms, so 48 to 240 samples).
2. Required gain per sample: g_req[n] = min(1, C / max(|x[n]|, eps)), where C is the ceiling (linear).
   For true-peak safety use the true-peak estimate of x (step 6) instead of |x|.
3. Sliding-window minimum of g_req over the last L samples (monotonic deque, O(1) amortized per sample). This makes the gain reach the needed value before the peak leaves the delay line.
4. Attack smoothing so the gain ramps down over the window instead of stepping: a moving-average (box) filter of length L applied to the windowed-min signal, or two cascaded box filters of length L/2 (smoother spectrum). Because the min window and the box filter have the same length, the smoothed gain is never above g_req at the moment the peak is output. This is the standard published construction.
5. Release: apply release before the window, on the required gain in dB. r[n] = min(g_req_dB[n], aR r[n-1] + (1 - aR) g_req_dB[n]), so r drops instantly and recovers toward 0 dB with time constant tau_R (10 ms to 1 s). Then do steps 3 and 4 on r. Optional program-dependent release as in section 1.
6. True-peak control: compute the 4x oversampled peak of the input (polyphase FIR, see below) for detection, or run the whole limiter at 4x. Detection-only oversampling is cheaper and good enough for vocals if the ceiling is set 0.3 to 1 dB below 0 dBTP.
7. Optional final safety clip at the ceiling (should almost never act).

True-peak measurement (BS.1770 Annex 2):

- Upsample by 4 (at 48 kHz) to 192 kHz using a polyphase low-pass interpolator. The reference filter in the Recommendation is a 48-tap FIR (4 phases of 12 taps). Formant can design its own windowed-sinc or equiripple polyphase filter with more taps for better stop-band rejection; the Recommendation allows other filters that meet the stated accuracy.
- Optional: attenuate by 12.04 dB before upsampling in fixed-point (not needed in float).
- Take absolute value of all four phases, maximum, then 20 log10, result in dBTP.
- 4x at 48 kHz can under-read inter-sample peaks by a fraction of a dB for content near Nyquist. Industry practice: ceiling -1 dBTP (EBU R 128 delivery max), -1 to -2 dBTP for lossy-codec delivery.
- Latency of a 48-tap 4x polyphase FIR is 6 input samples (0.125 ms) for detection.

### Classic behavior worth emulating

- Waves L1 style (public docs only): controls Threshold, Ceiling (Out Ceiling), Release, with auto release (ARC) and dither. Threshold drives the signal into a fixed ceiling, so "threshold" acts as input gain. This is the expected UI for a "maximizer".
- Orban-style broadcast: clipping plus overshoot compensation; mostly irrelevant for a vocal processor, but a soft clipper before the limiter (section 6) reduces how often the limiter acts.
- Most modern limiters expose Lookahead (0 to 5 ms), Attack (when lookahead is exposed), Release, Ceiling, True-peak toggle, and a "style" for release behavior.

### Recommended control set

| Control | Range | Default |
|---|---|---|
| Input gain / Threshold | -24 to 0 dB (threshold) or 0 to +24 dB (drive) | 0 dB |
| Ceiling | -12 to 0 dBFS / dBTP | -1.0 dB |
| Lookahead | 0 to 5 ms (0 = pure peak limiter with fast attack) | 1.5 ms (72 samples) |
| Release | 1 ms to 1 s | 50 ms |
| Auto release | off / on | on |
| True-peak | off / on (4x detection) | on |
| Gain reduction meter | | |

Note for live monitoring: each ms of lookahead is added to monitor latency. Offer a "zero latency" mode (lookahead 0, fast one-pole attack 0.05 ms plus hard clip safety) and show the added latency in the UI.

### Pitfalls

- Sliding minimum with a naive O(L) scan per sample is fine at L = 72 but a deque is cleaner and constant time.
- Attack smoothing with a one-pole filter instead of a finite window cannot guarantee the ceiling; peaks leak. Use the finite window or a final clip.
- Dry/wet on a limiter defeats the ceiling. Do not offer mix.
- Intersample peaks: a sample-peak limiter at 0 dBFS can output +1 to +3 dBTP on bright sibilant material.
- Two-stage limiter designs (slow stage plus delayed fast stage) are claimed in the active Dolby patent US 8,488,811. Do not combine a rate-limited pre-gain stage with a delayed second gain stage in one limiter.
- Block-based backward-pass gain contours resemble the active Mimi patent US 11,496,099. Formant works per sample, which avoids that structure.

---

## 3. Noise gate and downward expander

### References

- Zolzer, DAFX, ch. 4 (noise gate with hysteresis, hold, lookahead).
- Giannoulis et al. 2012: the same knee formula applies with an expander ratio.
- Reiss and McPherson, "Audio Effects", CRC 2014, ch. 6.
- Drawmer DS201 manual and spec sheet (public): two-channel gate, controls Threshold, Attack, Hold, Decay (release), Range, key HPF and LPF, Key Listen, Gate/Duck. https://www.drawmer.com (product manual)
- Shure and other automixer patents (US 4,658,425 "Microphone actuation control system", 1987, expired) show classic speech gating with noise-adaptive threshold. The 1987 patent is expired and is a good public-domain reference for gating speech mics against room noise.
- ACTIVE, avoid: US 2023/0215450 (VAD-set gate threshold), US 11,302,347 / 11,688,418 (VAD in automixer gating), VAD-based mute patents (US 9,002,030, US 10,230,346).

### Key algorithm details

Detector: peak or short RMS (1 to 5 ms), in dB, after an optional sidechain band filter (HPF 20 Hz to 4 kHz, LPF 250 Hz to 20 kHz).

Gate state machine with hysteresis and hold:

    open_th  = T
    close_th = T - H              (H = hysteresis, 2 to 6 dB)
    state OPEN   : if level < close_th: start hold counter
                   if hold counter expired: state = CLOSING
                   if level > open_th: reset hold
    state CLOSED : if level > open_th: state = OPEN (attack ramp starts)

Gain target: 1 when open, r = 10^(-Range/20) when closed. Smooth with separate attack (open) and release (close) one-poles in dB, or linear ramps for attack (click-free and predictable).

Downward expander static curve (ratio R_e >= 1, soft knee W, below threshold):

    if 2 (x_G - T) > W:          y_G = x_G
    if 2 |x_G - T| <= W:         y_G = x_G + (R_e - 1) (x_G - T - W/2)^2 / (2W)
    if 2 (x_G - T) < -W:         y_G = T + (x_G - T) R_e
    gain_dB = max(y_G - x_G, -Range)

A gate is the limit R_e to infinity with a range floor. For voice, an expander at 2:1 to 4:1 with 10 to 20 dB range sounds more natural than a hard gate.

Lookahead: delay audio by 0.5 to 5 ms so the attack ramp completes before the first consonant. Costs latency, so make it optional for live use.

### Classic behavior worth emulating

- Drawmer DS201 (approximate public ranges, verify against the manual): threshold about -54 to +20 dB; attack from microseconds to about 200 ms; hold about 2 ms to 2 s; decay about 2 ms to 4 s; range 0 to 80 dB; key HPF about 25 Hz to 4 kHz, key LPF about 250 Hz to 35 kHz; Key Listen solos the filtered sidechain. Very fast attack is its known strength (no click because of the program-adaptive attack slope).
- Broadcast and podcast gates (public docs of common plugins): Threshold, Range, Attack, Hold, Release, Hysteresis, Lookahead, sidechain filter with listen.

### Recommended control set

| Control | Range | Default (voice) |
|---|---|---|
| Mode | Gate / Expander | Expander |
| Threshold | -80 to 0 dBFS | -45 dB |
| Ratio (expander) | 1:1 to 10:1 | 3:1 |
| Range (depth) | 0 to 80 dB | 18 dB |
| Hysteresis | 0 to 12 dB | 4 dB |
| Attack | 0.05 to 50 ms | 1 ms |
| Hold | 0 to 500 ms | 60 ms |
| Release | 5 ms to 2 s | 150 ms |
| Lookahead | 0 to 5 ms | 0 ms |
| Sidechain HPF / LPF | 20 Hz to 4 kHz / 1 to 20 kHz | 100 Hz / 8 kHz |
| Sidechain listen | on / off | off |

### Pitfalls

- Chatter when level hovers near threshold. Hysteresis plus hold fixes it.
- Clicks from instant gain steps. Minimum attack ramp about 0.05 ms, or linear ramp of at least 2 samples.
- Breath and tail truncation: release too short cuts word endings. 100 to 250 ms is safe for speech.
- An automatic threshold that follows a measured noise floor during non-speech, gated by a VAD, is in active patent applications (section 0). A manual threshold with a visible noise-floor meter is safe.

---

## 4. De-esser

### References

- Zolzer, DAFX, ch. 4 and Reiss and McPherson ch. 6 (split-band dynamics).
- MathWorks example "Split-Band De-Essing" (public documentation): https://www.mathworks.com/help/audio/ug/SplitBandDeEssing.html
- Wikipedia, "De-esser" (overview of wideband vs split-band): https://en.wikipedia.org/wiki/De-esser
- US 6,373,953 B1 "Apparatus and method for de-esser using adaptive filtering algorithms". Expired.
- ACTIVE, avoid: Dolby US 11,322,170 / US 12,051,435 (FFT buffers, relative sibilance energy plus zero-crossing rate), Cerence US 11,817,115 (psychoacoustic sharpness as detector). Also recent "content-aware" filings (for example US 12,682,912). See section 0.

### Key algorithm details

Detector: sidechain band-pass or high-pass at the sibilance band (female 6 to 9 kHz, male 4 to 7 kHz, range 2 to 12 kHz), fast peak detector (attack 0.1 to 1 ms, release 20 to 80 ms).

Two detection modes, both classic:

1. Absolute: sidechain level in dB vs threshold. Simple; needs re-setting when input gain changes.
2. Relative (split-ratio): compare sibilance band level to full-band (or low-band) level, d = L_sib - L_full (dB). Reduce when d exceeds a threshold. This makes it level independent. Note: the active Dolby claim is specific (FFT buffers, previous-buffer envelope, ZCR test). A time-domain IIR band ratio is an older, published approach used in analog de-essers since the 1970s, but keep it time-domain and do not add a ZCR test.

Gain stage options:

- Wideband: apply the computed gain to the whole signal. Most transparent on harsh "s" but ducks the whole word slightly.
- Split-band: split with a complementary crossover (Linkwitz-Riley 4th order, or a complementary "x - HPF(x)" split with matched phase), compress only the high band, sum. Use LR4 so the bands sum flat in magnitude.
- Dynamic EQ: a peaking or high-shelf biquad (RBJ) at the sibilance frequency whose gain is driven by the detector (gain from 0 dB down to -Range). Cheapest and very transparent for vocals. Update coefficients per sample or per 8 to 16 samples with smoothed gain, or use a TPT/SVF form so coefficient changes do not click.

Gain computer: same as section 1 with ratio 4:1 to infinity and Range limit 0 to 20 dB.

### Classic behavior worth emulating

- Classic hardware (dbx 902, SPL De-Esser, channel-strip de-essers): Frequency, Threshold or Sensitivity, Range, Wide/Split mode, Listen.
- Popular plugin UIs (public docs): Frequency, Threshold, Range, Mode (wideband/split), Filter (band-pass/high-pass), Listen (audition sidechain), Absolute/Relative detection, Lookahead.

### Recommended control set

| Control | Range | Default |
|---|---|---|
| Frequency | 2 to 12 kHz | 6.5 kHz |
| Detector filter | Band-pass (Q 1 to 3) / High-pass | Band-pass Q 1.5 |
| Threshold | -60 to 0 dB (absolute) or 0 to +20 dB (relative) | -30 dB |
| Range | 0 to 20 dB | 8 dB |
| Mode | Wideband / Split / Dynamic EQ (shelf) | Split |
| Detection | Absolute / Relative | Relative |
| Attack / Release | 0.1 to 5 ms / 10 to 200 ms | 0.5 ms / 50 ms |
| Lookahead | 0 to 2 ms | 0 |
| Listen | sidechain audition | off |

### Pitfalls

- Split-band with a non-complementary filter (for example a plain 2nd-order HPF and LPF) causes a notch or bump at the crossover when no reduction happens. Use LR4 or the "subtract" topology.
- Fast attack on a wideband de-esser produces audible "lisping". Keep range modest.
- Frequency too low ducks "sh" and "ch" and dulls the voice; too high misses the energy at 5 to 6 kHz on male voices.
- Do not stack FFT-buffer analysis with ZCR tests (active Dolby claims), or a sharpness model (active Cerence claims).

---

## 5. EQ

### References

- R. Bristow-Johnson, "Cookbook formulae for audio EQ biquad filter coefficients" (Audio EQ Cookbook), now W3C Note: https://www.w3.org/TR/audio-eq-cookbook/ (public, free to use).
- M. Vicanek, "Matched Second Order Digital Filters", 2016: https://www.vicanek.de/articles/BiquadFits.pdf . Removes bilinear "cramping" near Nyquist with closed-form coefficients.
- M. Massberg, "Digital Low-Pass Filter Design with Analog-Matched Magnitude Response", AES 131st Convention, 2011, paper 8551.
- S. J. Orfanidis, "Digital Parametric Equalizer Design with Prescribed Nyquist-Frequency Gain", JAES 45(6), 1997.
- D. Bohn, Rane Note 101 "Constant-Q Graphic Equalizers" and Rane Note 115 "Operator Adjustable Equalizers: An Overview" (proportional-Q vs constant-Q). https://www.ranecommercial.com/legacy/note101.html and note115.
- V. Zavalishin, "The Art of VA Filter Design" (free PDF, Native Instruments, rev. 2.1.2, 2020): TPT/SVF forms for modulation-safe EQ.
- A. Simper (Cytomic), "Linear Trapezoidal Integrated State Variable Filter", free technical paper: SVF with cookbook-equivalent responses, stable under fast modulation.
- Pulse Techniques EQP-1A public manual and schematic (1950s, widely reproduced).

### Key algorithm details

RBJ biquad (Direct Form I or Transposed DF II in f64; for modulation use SVF):

    w0 = 2 pi f0 / fs,   cosw = cos(w0),   alpha = sin(w0) / (2 Q),   A = 10^(dBgain / 40)

    Peaking:  b0 = 1 + alpha A,  b1 = -2 cosw,  b2 = 1 - alpha A
              a0 = 1 + alpha / A, a1 = -2 cosw,  a2 = 1 - alpha / A

    Low shelf (S = shelf slope, alpha = sin(w0)/2 * sqrt((A + 1/A)(1/S - 1) + 2)):
              b0 =    A ((A+1) - (A-1) cosw + 2 sqrt(A) alpha)
              b1 =  2 A ((A-1) - (A+1) cosw)
              b2 =    A ((A+1) - (A-1) cosw - 2 sqrt(A) alpha)
              a0 =       (A+1) + (A-1) cosw + 2 sqrt(A) alpha
              a1 =   -2 ((A-1) + (A+1) cosw)
              a2 =       (A+1) + (A-1) cosw - 2 sqrt(A) alpha
    High shelf: swap the sign of the (A-1) cosw terms and the a1/b1 signs per the cookbook.

    Normalize by a0.  y = (b0 x + b1 x1 + b2 x2 - a1 y1 - a2 y2)

Cramping: the bilinear transform maps analog infinity to Nyquist, so bells above about 8 kHz at 48 kHz narrow and become asymmetric, and a high-shelf "air" boost at 16 kHz stops rising. Fixes:

1. Vicanek matched biquad: match the analog magnitude at DC, at f0, and at Nyquist in closed form. Same cost as RBJ at runtime; only the coefficient calculation changes.
2. Orfanidis prescribed Nyquist gain for peaking EQ.
3. Oversample 2x for the EQ (costs CPU and latency).

Proportional Q: the effective bandwidth narrows as boost or cut increases (API 550, many consoles). A simple model: Q_eff = Q * 10^(|gain_dB| / 40) or Q_eff = Q (1 + |gain_dB| / 12). Constant-Q: Q does not depend on gain (Rane Note 101). Offer a toggle; default proportional for "analog" feel.

Pultec EQP-1A (passive LC network plus make-up tube stage):

- Low section: frequency 20, 30, 60, 100 Hz; Boost 0 to 10 (up to about +13 dB) and Atten 0 to 10 (up to about -17 dB) as separate knobs at the same frequency.
- High boost: 3, 4, 5, 8, 10, 12, 16 kHz, peaking, with a Bandwidth (Sharp to Broad) control; boost up to about +16 dB.
- High atten: shelf at 5, 10, 20 kHz; up to about -16 dB.
- The "simultaneous boost and cut" trick on the low band: boost and attenuation shelves have different corner frequencies in the passive network, so using both gives a low-shelf boost with a dip just above it (for example at 100 Hz: rise below about 60 Hz, dip near 200 to 300 Hz). Emulate as low-shelf boost at f (slope gentle, Q about 0.5) plus a wider low-shelf cut whose corner is about 1.5 to 2.5 times higher, or as a shelf plus a broad peaking cut. Fit the curves to published measurements.
- Passive curves are broad (low Q shelves, Q about 0.4 to 0.6). Keep slopes gentle.

Analog-matched shelving: use Vicanek or Massberg forms for "air" shelves above 10 kHz.

Dynamic EQ: per band, a sidechain band-pass at the band frequency feeds a gain computer (section 1 or expander from section 3), and the result sets the band's gain in dB between the static gain and static gain plus or minus Range. Use SVF so per-sample gain changes are clean. The concept is long published (Zolzer DAFX; Reiss and McPherson). Do not copy a vendor's specific detector design without checking for patents.

### Classic behavior worth emulating

- Pultec EQP-1A: as above; three-knob simplicity, broad curves, mild tube coloration.
- API 550A: proportional-Q, stepped frequencies, +/-12 dB in 2 dB steps (public spec).
- Neve 1073: fixed HPF (50, 80, 160, 300 Hz, 18 dB/oct), low shelf 35 to 220 Hz, mid peak 360 Hz to 7.2 kHz, high shelf 12 kHz, gain +/-16 dB (public spec).

### Recommended control set (vocal EQ)

| Band | Type | Frequency | Gain | Q | Default |
|---|---|---|---|---|---|
| HPF | see section 9 | 20 to 400 Hz | n/a | 12/18/24 dB/oct | 80 Hz, 12 dB/oct |
| Low | Shelf / Bell | 30 to 500 Hz | +/-18 dB | 0.3 to 4 | 150 Hz shelf, 0 dB |
| Low-mid | Bell | 100 Hz to 2 kHz | +/-18 dB | 0.3 to 10 | 300 Hz, 0 dB, Q 1.0 |
| High-mid | Bell | 1 to 8 kHz | +/-18 dB | 0.3 to 10 | 3 kHz, 0 dB, Q 1.0 |
| High | Shelf / Bell | 2 to 20 kHz | +/-18 dB | 0.3 to 4 | 10 kHz shelf, 0 dB |
| Options | Proportional Q toggle; Analog-matched (decramped) toggle; Output gain +/-18 dB | | | | |

Plus a "Pultec" style module (separate preset or effect): Low freq 20/30/60/100 Hz, Low Boost, Low Atten, High Boost freq 3 to 16 kHz with Bandwidth, High Atten freq 5/10/20 kHz.

### Pitfalls

- Coefficients recomputed per sample from smoothed parameters with Direct Form structures can click or go unstable under fast sweeps. Use SVF/TPT for any modulated filter, and smooth parameters (one-pole, about 20 ms).
- Single precision Direct Form I at low frequencies (HPF at 20 Hz) has poor noise and pole precision. Use f64 state or SVF.
- Cramping above 8 kHz. Use matched design.
- Proportional-Q interacts with UI: show the actual curve, not the nominal Q.

---

## 6. Saturation, tape and tube

### References

- J. D. Parker, V. Zavalishin, E. Le Bivic, "Reducing the Aliasing of Nonlinear Waveshaping Using Continuous-Time Convolution", DAFx-16, Brno, 2016. https://dafx.de/paper-archive/details/vem_XXF5qBbfiWOH2RVVAA
- S. Bilbao, F. Esqueda, J. D. Parker, V. Valimaki, "Antiderivative Antialiasing for Memoryless Nonlinearities", IEEE Signal Processing Letters 24(7), 2017 (second-order ADAA).
- M. Vicanek, "Antialiased Distortion" notes: https://www.vicanek.de/articles/AADistortion.pdf
- J. Chowdhury, "Real-Time Physical Modelling for Analog Tape Machines", DAFx-19, Birmingham. https://dafx2019.bcu.ac.uk/papers/DAFx2019_paper_3.pdf . Reference code: https://github.com/jatinchowdhury18/AnalogTapeModel (GPLv3, license compatible with Formant; cite if used).
- D. C. Jiles, D. L. Atherton, "Theory of ferromagnetic hysteresis", J. Magnetism and Magnetic Materials 61, 1986 (the hysteresis model used by Chowdhury).
- J. Pakarinen, D. T. Yeh, "A Review of Digital Techniques for Modeling Vacuum-Tube Guitar Amplifiers", Computer Music Journal 33(2), 2009.
- N. Koren, "Improved VT models for SPICE simulations", Glass Audio 1996 (triode equations, public).
- D. Yeh, J. Abel, J. O. Smith, "Simplified, physically-informed models of distortion and overdrive guitar effects pedals", DAFx-07.

### Key algorithm details

Static waveshapers (choose by harmonic content):

    soft (odd only):    f(x) = tanh(k x) / tanh(k)
    cubic soft clip:    f(x) = x - x^3/3 for |x| <= 1, else sign(x) * 2/3
    asymmetric (adds even harmonics): f(x) = tanh(k (x + b)) - tanh(k b)    (b = 0.05 to 0.3)
    triode-like:        f(x) = x >= 0 ? tanh(k x) : (1 / k2) tanh(k2 x) with k2 < k  (grid-current side compresses harder)

Even harmonics come from asymmetry (b != 0 or different positive and negative curves). Odd harmonics come from symmetric curves. Always follow an asymmetric shaper with a DC blocker: y = x - x1 + R y1, R = 0.995 at 48 kHz (corner about 40 Hz) or lower R for higher corner.

First-order ADAA (Parker et al.):

    F = antiderivative of f
    y[n] = (F(x[n]) - F(x[n-1])) / (x[n] - x[n-1])          if |x[n] - x[n-1]| > eps
    y[n] = f((x[n] + x[n-1]) / 2)                            otherwise   (eps about 1e-5 in f64)

Antiderivatives:

    f = tanh:        F(x) = ln(cosh(x))   compute as |x| + ln(1 + exp(-2|x|)) - ln 2  (overflow-safe)
    f = hard clip:   F(x) = x^2 / 2 for |x| <= 1, else |x| - 1/2
    f = cubic:       F(x) = x^2/2 - x^4/12 for |x| <= 1, else (2/3)|x| - 1/4

ADAA adds a half-sample delay and a gentle low-pass (sinc-like, about -3.9 dB at Nyquist for first order). Compensate the low-pass with a small high-shelf if needed, and account for the half-sample delay when mixing dry and wet (or delay the dry path by a half-sample allpass, or accept it).

Second-order ADAA (Bilbao et al. 2017) needs the second antiderivative and has one sample of delay; better rejection at more CPU.

Oversampling: 2x with ADAA is a good trade for vocals; 4x without ADAA is roughly equivalent. Use a polyphase half-band FIR (linear phase, adds latency of about N/4 input samples for an N-tap half-band) or minimum-phase IIR half-band (Lagrange or elliptic allpass polyphase, near zero latency) for live monitoring.

Tape model (Chowdhury 2019), simplified pipeline:

1. Input gain and pre-emphasis (record EQ).
2. Hysteresis: Jiles-Atherton magnetization M(H) solved per sample with RK2 or RK4 (Chowdhury uses RK4 and 2x or more oversampling). Parameters: drive (input scale), saturation M_s, width (k), and a coercivity-like term.
3. Playback loss effects: spacing loss, thickness loss, gap loss, modeled as frequency-dependent filters depending on tape speed and head gap (Chowdhury gives the formulas).
4. De-emphasis and output gain.
5. Wow and flutter (see section 8) and optional hiss.

A cheaper "tape-ish" approximation for real time: high-shelf pre-emphasis, asymmetric soft saturation with mild hysteresis (one-pole state in the shaper argument), head-bump peaking EQ (about +2 dB around 50 to 100 Hz), high-frequency roll-off around 12 to 16 kHz, de-emphasis.

### Classic behavior worth emulating

- Tape: soft compression of peaks, head bump in lows, softened highs, low-order even plus odd harmonics, wow and flutter.
- Tube preamp: mostly 2nd harmonic at low drive, compression rising with drive, warmer low-mids.
- Transformer: low-frequency saturation (level and frequency dependent), which can be approximated by a saturator applied to a low-passed copy.

### Recommended control set

| Control | Range | Default |
|---|---|---|
| Drive | 0 to +36 dB | +6 dB |
| Type | Soft (tanh) / Hard / Tube (asymmetric) / Tape | Tube |
| Bias / Asymmetry | 0 to 1 (even harmonic amount) | 0.2 |
| Tone (post tilt or LPF) | 2 to 20 kHz | 16 kHz |
| Mix | 0 to 100 percent | 100 percent |
| Output | -24 to +12 dB, with auto gain compensation | auto |
| Quality | ADAA only / ADAA plus 2x oversampling | ADAA plus 2x |
| Tape (extra) | Speed 7.5/15/30 ips, Bias, Wow, Flutter, Hiss | 15 ips |

### Pitfalls

- ADAA division by near-zero differences: use the midpoint fallback and an absolute eps in f64.
- ADAA inside a feedback loop (for example in a delay feedback path) changes loop delay by a half sample; fine for delay, matters for filters.
- Auto gain on a saturator should be based on the static curve slope or a slow RMS match, not a fast follower (that would act as a compressor).
- Asymmetric shapers produce DC; always DC-block.
- Hysteresis with explicit Euler at 48 kHz without oversampling is unstable at high drive. Use RK4 or reduce drive range.

---

## 7. Reverb

### References

- M. R. Schroeder, "Natural Sounding Artificial Reverberation", JAES 10(3), 1962.
- J. A. Moorer, "About This Reverberation Business", Computer Music Journal 3(2), 1979.
- J. Stautner, M. Puckette, "Designing Multi-Channel Reverberators", Computer Music Journal 6(1), 1982 (first FDN form). Old prior art.
- J.-M. Jot, A. Chaigne, "Digital Delay Networks for Designing Artificial Reverberators", AES 90th Convention, 1991, preprint 3030.
- J.-M. Jot, A. Chaigne, US 5,491,754 "Method and system for artificial spatialisation of digital audio signals", France Telecom, priority 1992-03-03, filed 1993-02-19. Expired (est. 2013). https://patents.google.com/patent/US5491754A/en
- J.-M. Jot, S. Dicker, L. Dahl, US 6,188,769 B1 "Environmental reverberation processor", Creative Technology, priority 1998-11-13. Expired 2019. https://patents.google.com/patent/US6188769B1/en
- J. Dattorro, "Effect Design Part 1: Reverberator and Other Filters", JAES 45(9), 1997. https://ccrma.stanford.edu/~dattorro/EffectDesignPart1.pdf
- Jezar at Dreampoint, Freeverb (public domain C++ source, 2000).
- D. Griesinger, "Practical Processors and Programs for Digital Reverberation", AES 7th International Conference, 1989. Lexicon reverb architecture concepts (random modulation of delay taps, "spin and wander"). Griesinger's 1992 LARES "Electroacoustic system" patent and all Lexicon reverb patents from the 1980s and early 1990s are past their term.
- S. J. Schlecht, E. A. P. Habets, "On Lossless Feedback Delay Networks", IEEE TSP 2017, and Valimaki et al., "Fifty Years of Artificial Reverberation", IEEE TASLP 20(5), 2012 (survey).
- J. O. Smith, "Physical Audio Signal Processing", online book, CCRMA (FDN, Freeverb, Schroeder sections): https://ccrma.stanford.edu/~jos/pasp/
- ACTIVE, caution: Sony US 8,345,887 (FDN with matrix feedback and a multi-channel output subset, est. exp. 2030). Keep to the published Jot/Stautner forms.

### Key algorithm details

Comb gain for target decay (all designs): for a delay of m samples and reverb time T60,

    g = 10^(-3 m / (fs T60))

Frequency-dependent decay (Moorer, Jot): put a one-pole low-pass in each feedback loop, H(z) = g (1 - d) / (1 - d z^-1). Jot's design makes T60 at DC and at Nyquist both exact by setting per-line gain and pole from T60_low and T60_high.

Schroeder (1962): 4 parallel feedback combs (delays about 29.7, 37.1, 41.1, 43.7 ms), then 2 series allpasses (5.0 ms g 0.7, 1.7 ms g 0.7). Allpass: y = -g x + x[n-m] + g y[n-m]. Metallic, low density; historic reference.

Moorer (1979): early reflections from a tapped delay line (about 19 taps over 80 ms), 6 combs with low-pass in the loop, one allpass.

Freeverb (public domain), tunings at 44.1 kHz (scale by 48000/44100 and round to integers, prefer primes):

- 8 parallel low-pass-feedback combs: 1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617 samples.
- 4 series allpasses: 556, 441, 341, 225 samples, feedback 0.5.
- Room size maps to comb feedback: fb = room * 0.28 + 0.7. Damping maps to the comb low-pass: damp = damping * 0.4. Input gain 0.015.
- Stereo spread +23 samples on the right channel (not needed for mono).

Dattorro plate (1997), designed at fs = 29761 Hz; scale delays by 48000/29761 = 1.6129 for 48 kHz:

- Pre-delay, then bandwidth one-pole low-pass (bandwidth 0.9995).
- Input diffusion: 4 series allpasses, lengths 142, 107, 379, 277 (coefficients 0.75, 0.75, 0.625, 0.625).
- Tank: two cross-coupled halves. Each half: modulated allpass (decay diffusion 1 = 0.70, lengths 672 and 908, excursion up to 16 samples at about 1 Hz), delay (4453 and 4217), damping one-pole low-pass (damping 0.0005), gain "decay" (0.5 default), allpass (decay diffusion 2 = 0.50, lengths 1800 and 2656), delay (3720 and 3163), gain "decay", then into the other half.
- Output: the paper gives seven taps per output channel taken from the tank delays and allpasses, with signs. For mono, use the left tap set, or sum left and right sets.
- Decay diffusion 2 = clamp(decay + 0.15, 0.25, 0.50) in the paper.

FDN (Jot 1991-92, Stautner and Puckette 1982):

    s[n+1] = A * (diag(g_i H_i) * delay_outputs) + b x[n]
    y[n]   = c^T * delay_outputs + d x[n]

- N = 8 or 16 delay lines, lengths mutually prime, spread over about 30 to 100 ms (scale with "size").
- A orthogonal (lossless): Householder A = I - (2/N) 1 1^T (O(N)), or normalized Hadamard (fast Walsh-Hadamard, O(N log N)).
- Per-line gain g_i = 10^(-3 m_i / (fs T60)); per-line damping H_i as above.
- Tone correction filter at the output to flatten the coloration from frequency-dependent decay (Jot).
- Input diffusion: 2 to 4 allpasses before the FDN to raise early echo density.

Modulation in tails (Griesinger, Dattorro): slowly modulate 1 to 4 delay lengths (allpass or delay) by a few samples at 0.1 to 2 Hz, with independent or random LFOs. This breaks up modal ringing and metallic flutter. Use interpolated reads (section 8); linear interpolation is acceptable here because the excursion is small, but it slightly low-passes; cubic Hermite is cleaner.

### Classic behavior worth emulating

- EMT 140 plate: bright, dense, fast build-up, no distinct early reflections; Dattorro topology fits this.
- Lexicon 224/480 halls and plates: modulated tails, adjustable "bass multiply" (separate low-frequency decay), crossover, diffusion, pre-delay, size.
- Room: short decay (0.3 to 1.2 s), audible early reflections.

### Recommended control set

| Control | Range | Default (vocal) |
|---|---|---|
| Type | Room / Hall / Plate | Plate |
| Mix | 0 to 100 percent (or Dry/Wet levels) | 20 percent |
| Pre-delay | 0 to 200 ms | 20 ms |
| Decay (T60) | 0.2 to 10 s | 1.6 s |
| Size | 0 to 1 (scales delay lengths) | 0.6 |
| Damping (HF decay ratio) | 1 to 20 kHz or HF multiply 0.1 to 1.0 | 6 kHz |
| Low multiply (bass decay ratio) and crossover | 0.5 to 2.0, 100 to 1000 Hz | 1.0, 300 Hz |
| Diffusion | 0 to 1 | 0.7 |
| Modulation depth / rate | 0 to 1 / 0.1 to 3 Hz | 0.3 / 0.8 Hz |
| Wet low-cut / high-cut | 20 to 1000 Hz / 1 to 20 kHz | 200 Hz / 9 kHz |
| Freeze | on / off | off |

### Pitfalls

- Comb and FDN feedback with float denormals in the decaying tail: flush to zero.
- Changing delay lengths ("size") live without crossfading causes clicks and pitch glitches. Crossfade or apply only on release.
- Freeze (gain 1.0) with damping produces a decaying tone; set damping to off during freeze.
- Wet low-cut matters for vocals: reverb on low end muddies the voice.
- Metallic ringing: delays not mutually prime, or too few lines, or no modulation.
- Sony US 8,345,887 is active; avoid a design that matches its specific claimed structure (matrix transform with outputs subset forming a multichannel signal). A mono-output Jot/Stautner FDN from the published papers is the safe path.

---

## 8. Delay and chorus

### References

- J. Dattorro, "Effect Design Part 2: Delay-Line Modulation and Chorus", JAES 45(10), 1997. https://ccrma.stanford.edu/~dattorro/EffectDesignPart2.pdf
- T. I. Laakso, V. Valimaki, M. Karjalainen, U. K. Laine, "Splitting the Unit Delay: Tools for Fractional Delay Filter Design", IEEE Signal Processing Magazine 13(1), 1996.
- O. Niemitalo, "Polynomial Interpolators for High-Quality Resampling of Oversampled Audio", 2001 (free PDF; Hermite and optimal polynomial interpolators).
- J. O. Smith, "Physical Audio Signal Processing" (delay lines, interpolation, flanging, chorus).
- Zolzer, DAFX, ch. 2 (delay-based effects: vibrato, flanger, chorus, slapback, echo).
- Public manuals: Roland RE-201 Space Echo, Maestro Echoplex EP-3, Boss CE-1/CE-2 (chorus rate and depth ranges).

### Key algorithm details

Circular buffer delay with fractional read position D (samples). Interpolation options:

- Linear: y = (1 - f) x[i] + f x[i+1]. Cheap; low-pass effect varies with f, which causes amplitude modulation of highs under modulation. Fine for slow chorus at 48 kHz.
- Allpass (Thiran first order): y = a x[i] + x[i-1] - a y_prev, a = (1 - f)/(1 + f). Flat magnitude, but it has internal state, so fast changes of f cause transients and it is wrong for wide modulation (Dattorro discusses this). Use it only for fixed or very slowly varying delays (for example tuning in reverb tanks).
- Lagrange 3rd order (4 points): maximally flat at DC, good general choice.
- Cubic Hermite (Catmull-Rom, 4 points):

        c0 = x0
        c1 = 0.5 (x1 - x_m1)
        c2 = x_m1 - 2.5 x0 + 2 x1 - 0.5 x2
        c3 = 0.5 (x2 - x_m1) + 1.5 (x0 - x1)
        y  = ((c3 f + c2) f + c1) f + c0

  Recommended for chorus, flanger and tape wow. Same cost as Lagrange, continuous first derivative.

Echo with feedback, tape character:

    read  = interp(buffer, write_pos - D(n))
    fb    = sat(LPF(HPF(read))) * feedback        (HPF about 100 to 300 Hz, LPF 2 to 6 kHz, sat = tanh or soft clip)
    write: buffer[write_pos] = x + fb
    out   = dry + mix * read

D(n) = D0 (1 + wow(n) + flutter(n)):

- Wow: 0.3 to 2 Hz, depth about 0.1 to 0.5 percent of delay time, smoothed random or sine.
- Flutter: 5 to 20 Hz, depth 0.02 to 0.1 percent.
- Smooth tempo or time changes with a one-pole on D (tape "pitch slide" when time changes), or crossfade between two read heads for a clean change.

Chorus (Dattorro and Boss CE-style):

- Base delay 7 to 20 ms, LFO depth 1 to 5 ms (peak), rate 0.1 to 3 Hz, sine or triangle LFO.
- For a mono chorus, sum 2 or 3 voices with LFO phases offset (0, 120, 240 degrees) for a thicker sound without stereo.
- Dattorro "white chorus": feedback taken from a fixed tap and blended, mix about 0.7 dry, feedforward 1.0, blend -0.7 for a flat-ish response.
- Flanger: base 0.5 to 5 ms, depth up to base, feedback -0.95 to 0.95.

Vibrato (pitch modulation only): 100 percent wet, base 5 ms, depth 0.5 to 3 ms, rate 4 to 7 Hz.

### Classic behavior worth emulating

- Roland RE-201: three playback heads, Repeat Rate (tape speed), Intensity (feedback), with bass and treble tone in the echo path; self-oscillates at high intensity with saturation keeping it bounded.
- Echoplex EP-3: variable head position, warm feedback path with tube or transistor saturation.
- Boss CE-1/CE-2: BBD chorus with a single LFO, Rate and Depth only; BBD filtering darkens the wet signal (low-pass about 8 to 10 kHz).

### Recommended control set

Delay:

| Control | Range | Default |
|---|---|---|
| Time | 1 ms to 2 s (or tempo sync) | 280 ms |
| Feedback | 0 to 110 percent (above 100 percent only with saturation on) | 30 percent |
| Mix | 0 to 100 percent | 20 percent |
| Low-cut / High-cut in feedback | 20 to 1000 Hz / 1 to 20 kHz | 200 Hz / 5 kHz |
| Saturation (feedback path) | 0 to 1 | 0.2 |
| Wow / Flutter | 0 to 1 each | 0.1 / 0.05 |
| Mode | Digital / Tape | Tape |

Chorus:

| Control | Range | Default |
|---|---|---|
| Rate | 0.05 to 5 Hz | 0.6 Hz |
| Depth | 0 to 5 ms | 2 ms |
| Base delay | 5 to 30 ms | 12 ms |
| Voices | 1 to 3 | 2 |
| Feedback | -0.7 to 0.7 | 0 |
| Mix | 0 to 100 percent | 35 percent |
| Wet high-cut | 4 to 20 kHz | 10 kHz |

### Pitfalls

- Allpass interpolation under modulation: transients and zipper noise. Use Hermite or Lagrange.
- Reading past the write head when D becomes smaller than the interpolator half-width: clamp D to at least 2 samples for 4-point interpolation.
- Feedback over 100 percent without a saturator explodes. Clamp or require saturation.
- Changing time instantly causes clicks; slew D or crossfade.

---

## 9. High-pass filter

### References

- RBJ Audio EQ Cookbook (HPF, Q parameter).
- Zavalishin, "The Art of VA Filter Design" (TPT, SVF, Butterworth cascades).
- Vicanek, "Matched Second Order Digital Filters" (matched HPF/LPF without bilinear warping).
- S. H. Linkwitz, "Active Crossover Networks for Noncoincident Drivers", JAES 24(1), 1976 (Linkwitz-Riley).
- Rane Note 160, "Linkwitz-Riley Crossovers: A Primer" (public).

### Key algorithm details

Butterworth sections as cascaded 2nd-order (RBJ HPF with given Q) plus one 1st-order section for odd orders:

| Order | Slope | Sections and Q |
|---|---|---|
| 1 | 6 dB/oct | 1st-order only |
| 2 | 12 dB/oct | Q = 0.7071 |
| 3 | 18 dB/oct | 1st-order + Q = 1.0 |
| 4 | 24 dB/oct | Q = 0.5412, Q = 1.3066 |
| 6 | 36 dB/oct | Q = 0.5176, 0.7071, 1.9319 |
| 8 | 48 dB/oct | Q = 0.5098, 0.6013, 0.9000, 2.5629 |

Butterworth section Q values: Q_k = 1 / (2 cos(theta_k)), theta_k = (2k - 1) pi / (2N) for k = 1 .. floor(N/2).

Linkwitz-Riley order 2M = Butterworth order M squared. LR4 = two identical Butterworth 2nd-order sections (each Q = 0.7071); the LR4 HPF is -6 dB at fc, and LR4 LP plus HP sum to an allpass (flat magnitude). Use LR4 for band splits (de-esser, multiband), Butterworth for a plain vocal HPF (-3 dB at fc, flatter passband).

Bilinear vs matched: for an HPF at 20 to 400 Hz at 48 kHz, prewarped bilinear (the RBJ formulas already prewarp f0) is accurate; warping only matters near Nyquist, so matched designs mainly benefit low-pass and high shelves. For an HPF the important issue is precision: use f64 coefficients and state, or SVF/TPT, because the poles sit very close to z = 1.

1st-order TPT high-pass (zero-delay feedback, stable under modulation):

    g = tan(pi fc / fs)
    G = g / (1 + g)
    v = (x - s) * G
    lp = v + s
    s = lp + v
    hp = x - lp

### Recommended control set

| Control | Range | Default |
|---|---|---|
| Frequency | 20 to 500 Hz | 80 Hz |
| Slope | 6 / 12 / 18 / 24 / 36 / 48 dB/oct | 12 dB/oct (vocal), 24 for rumble |
| Resonance (optional, 2nd order only) | Q 0.5 to 2 | 0.707 |
| Type | Butterworth / Linkwitz-Riley / Bessel (gentle phase) | Butterworth |

### Pitfalls

- f32 Direct Form at 20 Hz: coefficient quantization and limit cycles; use f64 or SVF.
- High orders add phase rotation near the cutoff that can make a voice sound thin; 12 to 18 dB/oct is usual for vocals.
- Neve-style fixed HPF positions (50, 80, 160, 300 Hz at 18 dB/oct) are a good preset set.

---

## 10. Loudness metering and auto-gain

### References

- ITU-R BS.1770-5 (2023): K-weighting, mean square, gating. https://www.itu.int/rec/R-REC-BS.1770
- EBU R 128 (2023 revision): loudness normalization, target -23 LUFS, max true peak -1 dBTP.
- EBU Tech 3341: momentary (400 ms), short-term (3 s), integrated, EBU mode meter behavior.
- EBU Tech 3342: Loudness Range (LRA).
- AES TD1004 / AES77 (streaming loudness recommendations, -16 to -14 LUFS typical targets).
- libebur128 (MIT license) as a public reference implementation for cross-checking numbers.
- ACTIVE, caution: DTS US 8,315,398 (perceptual-loudness AGC, est. exp. 2030). Dolby has other loudness-leveling patents; older ones from 2003 to 2005 priority (Dolby Volume era) are expiring around now, so verify any specific one before copying it.

### Key algorithm details

K-weighting at 48 kHz (two biquads, coefficients published in BS.1770):

    Stage 1 (high shelf, about +4 dB above 2 kHz):
      b = [1.53512485958697, -2.69169618940638, 1.19839281085285]
      a = [1.0, -1.69065929318241, 0.73248077421585]
    Stage 2 (RLB high-pass, about 38 Hz):
      b = [1.0, -2.0, 1.0]
      a = [1.0, -1.99004745483398, 0.99007225036621]

For other sample rates derive the coefficients from the analog prototype (f0 about 1681.97 Hz, G about 3.9998 dB, Q about 0.7072 for stage 1; f0 about 38.135 Hz, Q about 0.5003 for stage 2) using the bilinear transform. Formant runs at 48 kHz, so use the table.

Mean square of the K-weighted signal z over a block:

    z_j = (1/T) * sum of y^2 over the block
    Loudness L = -0.691 + 10 log10(sum_i G_i z_i)   (mono: one channel, G = 1.0)

Mono note: BS.1770 treats a mono signal as one channel. Many platforms play mono as dual mono, which reads +3 dB louder on stereo meters. Decide and document which convention the target uses (most podcast tools measure mono files as dual-mono for the target; state it in the UI).

Integrated loudness gating:

1. Blocks of 400 ms with 75 percent overlap (step 100 ms).
2. Absolute gate: discard blocks with L_j < -70 LUFS.
3. Relative gate: compute the mean of remaining blocks, L_abs; discard blocks with L_j < L_abs - 10 LU.
4. Integrated loudness = -0.691 + 10 log10(mean of z_j over remaining blocks).

Momentary = ungated 400 ms window. Short-term = ungated 3 s window. LRA (Tech 3342): short-term values at 10 Hz or faster, absolute gate -70 LUFS, relative gate -20 LU, then LRA = 95th percentile minus 10th percentile.

Auto-gain / loudness target for a live vocal processor:

- Measure short-term loudness (3 s) or a custom 1 to 3 s K-weighted window.
- Speech gate: only update when the block is above an absolute floor (for example -50 LUFS) and within -10 to -20 LU of the running integrated value, so silence does not cause gain to rise ("breathing").
- Gain error e = target - L_short; apply gain with a slow slew (0.5 to 3 dB per second up, faster down), clamp total gain to a range (for example -12 to +12 dB).
- Place the auto-gain before the final limiter; the limiter enforces the true-peak ceiling.
- This combination is standard broadcast practice and is documented in EBU and AES material. Do not add a psychoacoustic loudness model beyond BS.1770 without reading the active DTS claims.

### Recommended control set

| Control | Range | Default |
|---|---|---|
| Target | -30 to -10 LUFS | -16 LUFS (streaming voice), -23 option (broadcast) |
| Max gain / Max cut | 0 to +24 dB / 0 to -24 dB | +12 / -12 |
| Speed | Slow / Medium / Fast (about 3, 1, 0.3 dB per s) | Medium |
| Freeze below | -70 to -30 LUFS | -50 LUFS |
| Meters | Momentary, Short-term, Integrated (reset), LRA, True peak | |

### Pitfalls

- Using momentary loudness for AGC causes pumping; use 1 to 3 s windows plus gating.
- Ungated integrated loudness drops in long silences; always apply the -70 LUFS absolute and -10 LU relative gates.
- Biquad K-weighting in f32 at 38 Hz is imprecise; use f64.
- Mono versus dual-mono convention can cause a 3 dB target error.

---

## 11. Vocal chain order and voice-activity gating

- No patent was found that claims the ordering of a vocal channel strip (HPF, gate, de-esser, EQ, compressor, saturation, delay, reverb, limiter). Console channel-strip patents found (Yamaha US 9,036,834, US 9,871,604) concern assigning physical strips to channels, not processing order. Standard ordering is documented in console manuals for decades (SSL 4000 channel strip: filters, dynamics, EQ, with a dynamics/EQ order switch).
- Conventional order for a live vocal: Input gain, HPF, Gate/Expander, (De-esser), EQ (corrective), Compressor, EQ (tone) or saturation, De-esser (if after compression), Delay and Reverb (as sends or parallel with wet-only processing), Auto-gain, Limiter (true-peak), Output.
- AI or "assistant" features that pick the chain or settings automatically from analysis are an active patent area for several plugin vendors (iZotope, Sonible, Waves and others have filings; not verified individually here). Run a patent search before shipping an automatic chain-configuration feature.
- Voice-activity-driven gating: classic energy plus hysteresis gating is old prior art (for example US 4,658,425, 1987, expired). Patents on specific VAD algorithms, VAD-set gate thresholds and VAD automixers are active (section 0). If Formant adds a VAD, prefer a published open algorithm, and keep the gate threshold user-set.

---

## 12. Implementation notes common to all effects

- Parameter smoothing: one-pole on every user parameter (about 10 to 30 ms) to avoid zipper noise; smooth in the parameter's natural domain (dB for gains, log frequency for cutoffs).
- Denormals: enable FTZ and DAZ on the audio thread (x86 MXCSR) or add an inaudible 1e-20 offset in feedback paths.
- Precision: f64 for filter state and coefficients below 100 Hz and for loudness accumulation; f32 is fine for delay buffers.
- Latency reporting: lookahead (limiter, gate, de-esser), linear-phase oversampling, and true-peak FIR all add latency. Sum and show it, and keep a "zero-latency monitoring" preset (no lookahead, minimum-phase oversampling or ADAA only).
- Time-constant convention: state in the UI whether attack and release are 63 percent (tau) or 10 to 90 percent times.
- Testing: impulse and sine-sweep tests for EQ curves against the analog prototype; static-curve tests (sweep level, read gain) for dynamics; BS.1770 compliance test signals in EBU Tech 3341 and 3342 for the loudness meter; aliasing tests with a 5 kHz to 20 kHz sine sweep through saturators.
