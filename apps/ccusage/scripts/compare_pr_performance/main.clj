(ns compare-pr-performance.main
  (:require [babashka.fs :as fs]
            [clojure.string :as str]
            [compare-pr-performance.benchmark :as benchmark]
            [compare-pr-performance.data :as data]
            [compare-pr-performance.report :as report]
            [compare-pr-performance.system :as system]))

(defn write-output! [output markdown]
  (if output (spit (system/expand output) markdown) (print markdown)))

(defn compare-command [variants command env options]
  (let [{:keys [rounds]} (benchmark/measure-command variants command env (assoc options :title "PR vs base"))]
    {:command command
     :base (benchmark/variant-summary rounds "base") :head (benchmark/variant-summary rounds "PR")
     :comparison (benchmark/compare-variants rounds "base" "PR" (:threshold options))}))

(defn github-warning!
  "GitHub reads workflow commands only from stdout, so the annotation must not
  go to stderr with the progress log."
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
                variants [{:label "base" :bin (system/expand (:base-bin options))}
                          {:label "PR" :bin (system/expand (:head-bin options))}]
                env (system/fixture-env fixture-dir codex-fixture-dir (str empty-home))
                results (mapv #(compare-command variants % env options) data/default-commands)
                context (assoc options :fixture-stats (system/summarize-directory fixture-dir)
                               :codex-fixture-stats (system/summarize-directory codex-fixture-dir))
                report (report/render-report context results {:base (system/optional-file-size (:bin (first variants)))
                                                       :head (system/optional-file-size (:bin (second variants)))})]
            (write-output! (:output options) report)
            (github-warning! (report/overall-verdict results (:threshold options)) (some? (System/getenv "GITHUB_ACTIONS"))))
          (finally (fs/delete-tree empty-home)))))))
