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

(def verdict-labels {:regression "slower" :improvement "faster" :unchanged "unchanged"})
(def rss-verdict-labels {:regression "more memory" :improvement "less memory" :unchanged "unchanged"})

(defn comparison-verdict [{:keys [cpu rss]}]
  (let [parts (remove nil? [(when-not (= :unchanged (:verdict cpu)) (verdict-labels (:verdict cpu)))
                            (when-not (= :unchanged (:verdict rss)) (rss-verdict-labels (:verdict rss)))])]
    (cond
      (or (= :regression (:verdict cpu)) (= :regression (:verdict rss))) (str "⚠️ " (str/join ", " parts))
      (seq parts) (str "✅ " (str/join ", " parts))
      :else "unchanged")))

(defn- verdicts [result] [(get-in result [:comparison :cpu :verdict]) (get-in result [:comparison :rss :verdict])])

(defn overall-verdict [results threshold]
  (let [percent (Math/round (* 100 threshold))
        regressions (filter #(some #{:regression} (verdicts %)) results)
        improvements (filter #(some #{:improvement} (verdicts %)) results)
        commands (fn [rs] (str/join ", " (map #(format "`%s` (%s)" (:command %) (str/replace (comparison-verdict (:comparison %)) #"^\S+ " "")) rs)))]
    (cond
      (seq regressions) {:regression? true
                         :text (format "**Verdict: ⚠️ possible regression** in %s; CPU time or peak RSS above +%d%%, consistent across rounds."
                                       (commands regressions) percent)}
      (seq improvements) {:text (format "**Verdict: ✅ improved** in %s; nothing regressed beyond %d%%." (commands improvements) percent)}
      :else {:text (format "**Verdict: no significant change** (every command within ±%d%% CPU time and peak RSS)." percent)})))

(defn fixture-line [{:keys [fixture-stats codex-fixture-stats grok-fixture-stats]}]
  (format "Fixture: generated Claude logs (%s on disk, %d files)%s Codex logs (%s on disk, %d files, mostly zstd-compressed)%s shaped like current real logs; every value is synthetic."
          (data/format-data-size (:bytes fixture-stats)) (:files fixture-stats)
          (if grok-fixture-stats "," " and")
          (data/format-data-size (:bytes codex-fixture-stats)) (:files codex-fixture-stats)
          (if grok-fixture-stats
            (format ", and Grok logs (%s on disk, %d files)" (data/format-data-size (:bytes grok-fixture-stats)) (:files grok-fixture-stats))
            "")))

(defn method-line [{:keys [runs warmup threshold]}]
  (format (str "Base and PR are built from source the same way on this runner and run `--offline --json` in %d interleaved rounds after %d warmup rounds; "
               "each round runs both binaries once in alternating order. Cells show median (min–max). "
               "Ratios are PR/base per round, so below 1.00 is better. A command is flagged when the median CPU-time or peak-RSS ratio moves past %d%% "
               "and at least four in five rounds agree. Wall time is shown but not judged.")
          runs warmup (Math/round (* 100 threshold))))

(defn comparison-rows [results]
  (let [instructions? (every? #(get-in % [:comparison :instructions]) results)]
    (mapv (fn [{:keys [command base head comparison]}]
            (apply array-map
                   (concat ["Command" (str "`" command "`")
                            "CPU base" (data/format-duration (get-in base [:cpu :median]))
                            "CPU PR" (data/format-duration (get-in head [:cpu :median]))
                            "CPU ratio" (format-ratio-spread (:cpu comparison))
                            "Wall base" (format-spread (:wall base) data/format-duration)
                            "Wall PR" (format-spread (:wall head) data/format-duration)
                            "RSS base" (data/format-memory-size (get-in base [:rss :median]))
                            "RSS PR" (data/format-memory-size (get-in head [:rss :median]))
                            "RSS ratio" (format-ratio-spread (:rss comparison))]
                           (when instructions? ["Instructions ratio" (format-ratio-spread (:instructions comparison))])
                           ["Verdict" (comparison-verdict comparison)])))
          results)))

(defn comment-marker [head-sha] (str "<!-- ccusage-perf-comment:" head-sha " -->"))

(defn render-report [{:keys [base-sha head-sha] :as context} results sizes]
  (str (str/join "\n" (concat (when head-sha [(comment-marker head-sha)])
                              ["## ccusage performance comparison" ""]
                              (when head-sha [(str "PR: `" (data/format-sha head-sha) "`"
                                                   (when base-sha (str " merged onto Base: `" (data/format-sha base-sha) "`")))
                                              ""])
                              [(:text (overall-verdict results (:threshold context))) ""
                               (fixture-line context) "" (method-line context) ""
                               (table-md (comparison-rows results)) ""
                               "### Binary size" ""
                               (table-md [(array-map "Artifact" "release binary" "Base" (data/format-optional-size (:base sizes))
                                                     "PR" (data/format-optional-size (:head sizes))
                                                     "Delta" (data/format-size-delta (:base sizes) (:head sizes)))])
                               ""]))
       "\n"))
