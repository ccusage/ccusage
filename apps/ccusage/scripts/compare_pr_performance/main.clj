(defn write-output! [output markdown]
  (if output (spit (expand output) markdown) (print markdown)))

(defn compare-command [variants command env options]
  (let [{:keys [rounds]} (measure-command variants command env (assoc options :title "PR vs base"))]
    {:command command
     :base (variant-summary rounds "base") :head (variant-summary rounds "PR")
     :comparison (compare-variants rounds "base" "PR" (:threshold options))}))

(defn github-warning!
  "GitHub reads workflow commands only from stdout, so the annotation must not
  go to stderr with the progress log."
  [verdict github-actions?]
  (when (and (:regression? verdict) github-actions?)
    (println (str "::warning title=ccusage performance::" (str/replace (:text verdict) "**" "")))))

(defn -main [& argv]
  (let [options (parse-cli argv)]
    (if (:help options)
      (println (help-text))
      (let [empty-home (temp-dir "ccusage-perf-home.")]
        (try
          (let [fixture-dir (expand (:fixture-dir options))
                codex-fixture-dir (expand (:codex-fixture-dir options))
                variants [{:label "base" :bin (expand (:base-bin options))}
                          {:label "PR" :bin (expand (:head-bin options))}]
                env (fixture-env fixture-dir codex-fixture-dir (str empty-home))
                results (mapv #(compare-command variants % env options) default-commands)
                context (assoc options :fixture-stats (summarize-directory fixture-dir)
                               :codex-fixture-stats (summarize-directory codex-fixture-dir))
                report (render-report context results {:base (optional-file-size (:bin (first variants)))
                                                       :head (optional-file-size (:bin (second variants)))})]
            (write-output! (:output options) report)
            (github-warning! (overall-verdict results (:threshold options)) (some? (System/getenv "GITHUB_ACTIONS"))))
          (finally (fs/delete-tree empty-home)))))))
