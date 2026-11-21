(ns compare-pr-performance.report
  (:require [clojure.string :as str]
            [compare-pr-performance.data :as data]))

(defn escape-cell [value] (-> (str value) (str/replace "|" "\\|") (str/replace "\n" "<br>")))
(defn table-md [rows]
  (let [headers (keys (first rows)) row-values (fn [row] (map #(escape-cell (get row %)) headers))]
    (str "| " (str/join " | " (map escape-cell headers)) " |\n"
         "| " (str/join " | " (repeat (count headers) "---")) " |\n"
         (str/join "\n" (map #(str "| " (str/join " | " (row-values %)) " |") rows)))))

(defn format-spread [{:keys [median min max]} formatter]
  (format "%s (%s–%s)" (formatter median) (formatter min) (formatter max)))
(defn format-ratio-spread [summary] (format-spread summary #(format "%.2f" (double %))))

(def verdict-labels
  {:wall {:regression "slower" :improvement "faster"}
   :cpu {:regression "more CPU" :improvement "less CPU"}
   :rss {:regression "more memory" :improvement "less memory"}})

(defn comparison-verdict [comparison]
  (let [judged (keep (fn [metric] (when-let [verdict (get-in comparison [metric :verdict])] [metric verdict])) [:wall :cpu :rss])
        parts (keep (fn [[metric verdict]] (get-in verdict-labels [metric verdict])) judged)]
    (cond
      (some #(= :regression (second %)) judged) (str "⚠️ " (str/join ", " parts))
      (seq parts) (str "✅ " (str/join ", " parts))
      :else "unchanged")))

(defn- verdicts [result] (keep #(get-in result [:comparison % :verdict]) [:wall :cpu :rss]))

(defn- result-name [{:keys [command fixture-label]}]
  (str "`" command "`" (when fixture-label (str " on " fixture-label))))

(defn- percent [ratio] (Math/round (* 100 ratio)))

(defn overall-verdict [results {:keys [threshold wall-threshold]}]
  (let [cpu-percent (percent threshold) wall-percent (percent wall-threshold)
        regressions (filter #(some #{:regression} (verdicts %)) results)
        improvements (filter #(some #{:improvement} (verdicts %)) results)
        commands (fn [rs] (str/join ", " (map #(format "%s (%s)" (result-name %) (str/replace (comparison-verdict (:comparison %)) #"^\S+ " "")) rs)))]
    (cond
      (seq regressions) {:regression? true
                         :text (format "**Verdict: ⚠️ possible regression** in %s; wall time above +%d%%, CPU time or peak RSS above +%d%%, consistent across rounds."
                                       (commands regressions) wall-percent cpu-percent)}
      (seq improvements) {:text (format "**Verdict: ✅ improved** in %s; nothing regressed beyond the thresholds." (commands improvements))}
      :else {:text (format "**Verdict: no significant change** (every command within ±%d%% wall time and ±%d%% CPU time and peak RSS)." wall-percent cpu-percent)})))

(defn fixture-line [{:keys [fixture-stats codex-fixture-stats grok-fixture-stats]}]
  (format "Fixture: generated Claude logs (%s on disk, %d files)%s Codex logs (%s on disk, %d files, mostly zstd-compressed)%s shaped like current real logs; every value is synthetic."
          (data/format-data-size (:bytes fixture-stats)) (:files fixture-stats)
          (if grok-fixture-stats "," " and")
          (data/format-data-size (:bytes codex-fixture-stats)) (:files codex-fixture-stats)
          (if grok-fixture-stats
            (format ", and Grok logs (%s on disk, %d files)" (data/format-data-size (:bytes grok-fixture-stats)) (:files grok-fixture-stats))
            "")))

(defn method-line [{:keys [runs warmup threshold wall-threshold]}]
  (format (str "Base and PR are built from source the same way on this runner and run `--offline --json` in %d interleaved rounds after %d warmup rounds; "
               "each round runs both binaries once in alternating order. Wall time, CPU time, and peak RSS come from the shell's `time` and GNU `time` around each run; instruction counts come from one extra hyperfine pass. Cells show median (min–max). "
               "Ratios are PR/base per round, so below 1.00 is better. A command is flagged when the median CPU-time or peak-RSS ratio moves past %d%% "
               "and at least four in five rounds agree; the same holds when the median wall-time ratio moves past %d%%.")
          runs warmup (percent threshold) (percent wall-threshold)))

(defn shape-lines [shapes]
  (map (fn [{:keys [label desc stats]}]
         (if stats
           (format "- **%s** (%s on disk, %d files): %s." label (data/format-data-size (:bytes stats)) (:files stats) desc)
           (format "- **%s**: the fixture above. Each file's mtime is its last record, so `--since` the day of the median mtime skips about half of the Claude files and `--since` the 95th-percentile day skips nearly all of them by mtime alone, and one Codex rollout in eight is a fork of the one before it." label)))
       shapes))

(defn comparison-rows [results]
  (let [instructions? (every? #(get-in % [:comparison :instructions]) results)]
    (mapv (fn [{:keys [command fixture-label base head comparison]}]
            (apply array-map
                   (concat (when fixture-label ["Fixture" fixture-label])
                           ["Command" (str "`" command "`")
                            "CPU base" (data/format-duration (get-in base [:cpu :median]))
                            "CPU PR" (data/format-duration (get-in head [:cpu :median]))
                            "CPU ratio" (format-ratio-spread (:cpu comparison))
                            "Wall base" (format-spread (:wall base) data/format-duration)
                            "Wall PR" (format-spread (:wall head) data/format-duration)
                            "Wall ratio" (format-ratio-spread (:wall comparison))
                            "RSS base" (data/format-memory-size (get-in base [:rss :median]))
                            "RSS PR" (data/format-memory-size (get-in head [:rss :median]))
                            "RSS ratio" (format-ratio-spread (:rss comparison))]
                           (when instructions? ["Instructions ratio" (format-ratio-spread (:instructions comparison))])
                           ["Verdict" (comparison-verdict comparison)])))
          results)))

(defn comment-marker [head-sha] (str "<!-- ccusage-perf-comment:" head-sha " -->"))

(defn render-report [{:keys [base-sha head-sha shapes] :as context} results shape-results sizes]
  (str (str/join "\n" (concat (when head-sha [(comment-marker head-sha)])
                              ["## ccusage performance comparison" ""]
                              (when head-sha [(str "PR: `" (data/format-sha head-sha) "`"
                                                   (when base-sha (str " merged onto Base: `" (data/format-sha base-sha) "`")))
                                              ""])
                              [(:text (overall-verdict (concat results shape-results) context)) ""
                               (fixture-line context) "" (method-line context) ""
                               (table-md (comparison-rows results)) ""]
                              (when (seq shape-results)
                                (concat ["### Targeted shapes" ""
                                         "Commands and fixtures that reach code paths the table above does not:" ""]
                                        (shape-lines shapes)
                                        ["" (table-md (comparison-rows shape-results)) ""]))
                              ["### Binary size" ""
                               (table-md [(array-map "Artifact" "release binary" "Base" (data/format-optional-size (:base sizes))
                                                     "PR" (data/format-optional-size (:head sizes))
                                                     "Delta" (data/format-size-delta (:base sizes) (:head sizes)))])
                               ""]))
       "\n"))
