(defn format-fixture-path [head-dir fixture-dir]
  (let [head (fs/canonicalize head-dir) fixture (fs/canonicalize fixture-dir)]
    (try (let [relative (str (fs/relativize head fixture))]
           (if (str/starts-with? relative "..") fixture-dir relative))
         (catch Exception _ fixture-dir))))
(defn format-fixture-stats [stats] (format "%s, %d files" (format-data-size (:bytes stats)) (:files stats)))
(defn fixture-stats-for-command [section command]
  (if (and (str/starts-with? command "codex") (:codex-fixture-stats section))
    (:codex-fixture-stats section) (:fixture-stats section)))
(defn escape-cell [value] (-> (str value) (str/replace "|" "\\|") (str/replace "\n" "<br>")))
(defn escape-header [value] (str/replace (escape-cell value) "/" "&#x2f;"))
(defn table-md [rows]
  (let [headers (keys (first rows)) row-values (fn [row] (map #(escape-cell (get row %)) headers))]
    (str "| " (str/join " | " (map #(escape-header (name %)) headers)) " |\n"
         "| " (str/join " | " (repeat (count headers) "---")) " |\n"
         (str/join "\n" (map #(str "| " (str/join " | " (row-values %)) " |") rows)))))

(defn fixture-line [section options]
  (if-not (:codex-fixture-dir section)
    (format "Fixture: `%s` (%s)" (format-fixture-path (:head-dir options) (:fixture-dir section))
            (format-fixture-stats (:fixture-stats section)))
    (format "Fixtures: Claude `%s` (%s), Codex `%s` (%s)"
            (format-fixture-path (:head-dir options) (:fixture-dir section)) (format-fixture-stats (:fixture-stats section))
            (format-fixture-path (:head-dir options) (:codex-fixture-dir section))
            (format-fixture-stats (or (:codex-fixture-stats section) (:fixture-stats section))))))
(defn render-fixture-section [section options]
  (let [has-memory (some #(or (:base-memory %) (:head-memory %)) (:results section))
        base-desc (or (:base-runtime-description options) "Base runs the package `ccusage` bin from `apps/ccusage/package.json` with Node")
        head-desc (or (:head-runtime-description options)
                      (if (= "rust" (:head-runtime options)) "PR runs `rust/target/release/ccusage` directly"
                          "PR runs the package `ccusage` bin from `apps/ccusage/package.json` with Node"))
        rows (mapv (fn [result]
                     (let [stats (fixture-stats-for-command section (:command result))
                           common ["Command" (str "`" (:command result) " --offline --json`")
                                   "Input" (format-data-size (:bytes stats))
                                   "Base median" (format-duration (get-in result [:base :median]))
                                   "PR median" (format-duration (get-in result [:head :median]))
                                   "PR vs base" (format "%.2fx" (/ (double (get-in result [:base :median])) (get-in result [:head :median])))]
                           memory (when has-memory
                                    ["Base peak RSS" (format-optional-memory (:base-memory result))
                                     "PR peak RSS" (format-optional-memory (:head-memory result))
                                     "PR/base RSS" (format-memory-ratio (:base-memory result) (:head-memory result))])
                           throughput ["Base throughput" (format-throughput (:bytes stats) (get-in result [:base :median]))
                                       "PR throughput" (format-throughput (:bytes stats) (get-in result [:head :median]))]]
                       (apply array-map (concat common memory throughput)))) (:results section))]
    (str/join "\n" (concat [(str "## " (:title section)) "" (:description section) "" (fixture-line section options)
                              (format "%s; %s. Both run `--offline --json`, measured by `hyperfine` with `%d` warmups and `%d` runs."
                                      base-desc head-desc (:warmup section) (:runs section))]
                             (when has-memory [(format "Peak RSS is measured separately with `/usr/bin/time` using `%d` runs. Lower RSS ratios are better." (:memory-runs section))])
                             ["" (table-md rows)]))))

(defn render-runtime-section [section options]
  (let [rows (mapv (fn [result] (let [stats (fixture-stats-for-command section (:command result))]
                                  (array-map "Command" (str "`" (:command result) " --offline --json`") "Runtime" (:label result)
                                             "Input" (format-data-size (:bytes stats)) "Median" (format-duration (get-in result [:measurement :median]))
                                             "Throughput" (format-throughput (:bytes stats) (get-in result [:measurement :median]))
                                             "Samples" (get-in result [:measurement :samples])))) (:results section))]
    (str/join "\n" [(str "## " (:title section)) "" (:description section) "" (fixture-line section options)
                      (format "All rows run `--offline --json`, measured by `hyperfine` with `%d` warmups and `%d` runs. This isolates wrapper overhead from the installed native optional dependency and the workspace release binary built on the runner." (:warmup section) (:runs section))
                      "" (table-md rows)])))

(defn marker-name [runtime] (if (= runtime "rust") "ccusage-rust-perf-comment" "ccusage-perf-comment"))
(defn render-skipped-markdown [{:keys [base-package-url base-sha head-runtime head-sha reason]}]
  (let [marker (marker-name head-runtime)]
    (str (str/join "\n" (concat [(str "<!-- " marker " -->")]
                                (when head-sha [(str "<!-- " marker ":" head-sha " -->")])
                                ["## ccusage performance comparison" ""]
                                (when head-sha (concat [(str "PR SHA: `" (format-sha head-sha) "`")]
                                                       (when base-sha [(str "Base SHA: `" (format-sha base-sha) "`")]) [""]))
                                ["Performance comparison skipped." "" reason]
                                (when base-package-url ["" (str "Base package: `" (package-url-sha base-package-url) "`")]) [""])) "\n")))

(defn render-report [context sections sizes]
  (let [marker (marker-name (:head-runtime context))
        size-rows (cond-> [(array-map "Artifact" "packed `ccusage-*.tgz`" "Base" (format-size (:base-package sizes))
                                     "PR" (format-size (:head-package sizes)) "Delta" (format-size-delta (:base-package sizes) (:head-package sizes))
                                     "Ratio" (format-size-ratio (:base-package sizes) (:head-package sizes)))]
                    (or (:base-native-package-binary sizes) (:head-native-package-binary sizes))
                    (conj (array-map "Artifact" "installed native package binary" "Base" (format-optional-size (:base-native-package-binary sizes))
                                     "PR" (format-optional-size (:head-native-package-binary sizes))
                                     "Delta" (format-size-delta (:base-native-package-binary sizes) (:head-native-package-binary sizes))
                                     "Ratio" (format-size-ratio (:base-native-package-binary sizes) (:head-native-package-binary sizes))))
                    (:head-rust-binary sizes)
                    (conj (array-map "Artifact" "Rust release binary `rust/target/release/ccusage`" "Base" (format-optional-size (:base-rust-binary sizes))
                                     "PR" (format-size (:head-rust-binary sizes)) "Delta" (format-size-delta (:base-rust-binary sizes) (:head-rust-binary sizes))
                                     "Ratio" (format-size-ratio (:base-rust-binary sizes) (:head-rust-binary sizes)))))]
    (str (str/join "\n" (concat [(str "<!-- " marker " -->")]
                                (when (:head-sha context) [(str "<!-- " marker ":" (:head-sha context) " -->")])
                                ["## ccusage performance comparison" ""]
                                (when (:head-sha context) (concat [(str "PR SHA: `" (format-sha (:head-sha context)) "`")]
                                                                  (when (:base-sha context) [(str "Base SHA: `" (format-sha (:base-sha context)) "`")]) [""]))
                                [(if (= "rust" (:head-runtime context))
                                   "This compares the Rust PR release binary against the configured base package on the same CI runner."
                                   "This compares the PR package against the configured base package on the same CI runner.") ""]
                                (mapcat #(vector (render-runtime-section % context) "") (:runtime-diagnostic-sections context))
                                (mapcat #(vector (render-fixture-section % context) "") sections)
                                ["## Artifact size" "" (table-md size-rows) ""
                                 "Lower medians and smaller artifacts are better. CI runner noise still applies; use same-run ratios as directional PR feedback, not release guarantees." ""])) "\n")))
