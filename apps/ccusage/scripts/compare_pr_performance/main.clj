(ns compare-pr-performance.main
  (:require [babashka.fs :as fs]
            [clojure.string :as str]
            [compare-pr-performance.benchmark :as benchmark]
            [compare-pr-performance.data :as data]
            [compare-pr-performance.report :as report]
            [compare-pr-performance.system :as system]))

(defn compare-command [variants command env options]
  (let [{:keys [rounds]} (benchmark/measure-command variants command env (assoc options :title (str "PR vs base" (some->> (:fixture-label options) (str " / ")))))]
    {:command command
     :base (benchmark/variant-summary rounds "base") :head (benchmark/variant-summary rounds "PR")
     :comparison (benchmark/compare-variants rounds "base" "PR" (:threshold options))}))

;; NOTE(2026-11-10, #2061): GitHub Actions reads workflow commands from stdout. Emit annotations there even though progress uses stderr.
(defn github-warning!
  [verdict github-actions?]
  (when (and (:regression? verdict) github-actions?)
    (println (str "::warning title=ccusage performance::" (str/replace (:text verdict) "**" "")))))

(defn -main [& argv]
  (let [options (data/parse-cli argv)]
    (if (:help options)
      (println (data/help-text))
      (let [empty-home (system/temp-dir "ccusage-perf-home.")]
        (try
          (let [fixture-dir (system/expand (:fixture-dir options))
                codex-fixture-dir (system/expand (:codex-fixture-dir options))
                grok-fixture-dir (some-> (:grok-fixture-dir options) system/expand)
                variants [{:label "base" :bin (system/expand (:base-bin options))}
                          {:label "PR" :bin (system/expand (:head-bin options))}]
                env (system/fixture-env fixture-dir codex-fixture-dir grok-fixture-dir (str empty-home))
                shapes (into {:large {:fixture :large :label "main" :env env :claude-dir fixture-dir}}
                             (keep (fn [{:keys [fixture option] :as shape}]
                                     (when-let [dir (some-> (get options option) system/expand)]
                                       [fixture (assoc shape :env (system/shape-env shape dir (str empty-home)) :claude-dir dir
                                                       :stats (system/summarize-directory dir))])))
                             data/shape-fixtures)
                results (mapv #(compare-command variants % env options) data/default-commands)
                shape-results (vec (for [{:keys [fixture command]} data/shape-commands
                                         :let [shape (get shapes fixture)]
                                         :when shape]
                                     (assoc (compare-command variants (system/resolve-command command (:claude-dir shape)) (:env shape)
                                                             (assoc options :fixture-label (:label shape)))
                                            :fixture-label (:label shape))))
                context (assoc options :fixture-stats (system/summarize-directory fixture-dir)
                               :codex-fixture-stats (system/summarize-directory codex-fixture-dir)
                               :grok-fixture-stats (some-> grok-fixture-dir system/summarize-directory)
                               :shapes (into [(:large shapes)] (keep #(get shapes (:fixture %))) data/shape-fixtures))
                report (report/render-report context results shape-results
                                             {:base (system/optional-file-size (:bin (first variants)))
                                              :head (system/optional-file-size (:bin (second variants)))})]
            (system/write-output! (:output options) report)
            (github-warning! (report/overall-verdict (into results shape-results) (:threshold options)) (some? (System/getenv "GITHUB_ACTIONS"))))
          (finally (fs/delete-tree empty-home)))))))
