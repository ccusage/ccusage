(def diff-spec
  {:real {:coerce :boolean :desc "Read the caller's real local agent logs instead of a hermetic environment"}
   :fixture {:ref "DIR" :desc "Fixture root; each existing <DIR>/<agent> becomes that agent's data directory"}
   :allow-stderr-diff {:coerce :boolean :desc "Compare stdout and exit code only"}
   :since {:coerce :string :default "20250101" :ref "YYYYMMDD" :desc "Start of the date-window variant (fixture mode)"}
   :until {:coerce :string :default "20251231" :ref "YYYYMMDD" :desc "End of the date-window variant (fixture mode)"}
   :jobs {:coerce :long :default 1 :ref "N" :desc "Commands compared in parallel; above 1 some adapters stop being reproducible on real logs"}
   :output-dir {:ref "DIR" :desc "Where outputs of differing commands are kept (default: a private temp dir)"}
   :cache-dir {:ref "DIR" :desc "Built binaries and shared cargo target (default: rust/target/bench-history)"}
   :help {:alias :h :coerce :boolean}})

(def agent-env-vars
  "Directory name under --fixture and the variable that points the agent at it."
  (array-map "claude" "CLAUDE_CONFIG_DIR" "codex" "CODEX_HOME" "opencode" "OPENCODE_DATA_DIR"
             "cline" "CLINE_DATA_DIR" "amp" "AMP_DATA_DIR" "droid" "DROID_SESSIONS_DIR"
             "codebuff" "CODEBUFF_DATA_DIR" "hermes" "HERMES_HOME" "pi" "PI_AGENT_DIR"
             "goose" "GOOSE_PATH_ROOT" "kilo" "KILO_DATA_DIR" "copilot" "COPILOT_HOME"
             "gemini" "GEMINI_DATA_DIR" "antigravity" "ANTIGRAVITY_DATA_DIR" "kimi" "KIMI_DATA_DIR"
             "qwen" "QWEN_DATA_DIR" "openclaw" "OPENCLAW_DIR" "grok" "GROK_HOME" "zcode" "ZCODE_HOME"))

(def period-reports ["daily" "monthly" "session"])
(def weekly-reports ["daily" "weekly" "monthly" "session"])

(defn agent-reports
  "Mirrors agent_report_supported in rust/crates/ccusage-cli-parser/src/parser.rs.
  statusline is left out: it reads a hook payload from stdin instead of logs."
  [agent]
  (case agent
    "claude" (conj weekly-reports "blocks")
    ("opencode" "cline") weekly-reports
    period-reports))

(def report-commands
  (vec (concat weekly-reports
               (for [agent (keys agent-env-vars) report (agent-reports agent)] (str agent " " report)))))

(def output-variants
  [{:name "json" :args ["--json"]}
   {:name "table" :args []}
   {:name "json-breakdown" :args ["--json" "--breakdown"]}
   {:name "json-window" :args ["--json"] :window? true}
   {:name "json-mode-calculate" :args ["--json" "--mode" "calculate"]}
   {:name "json-mode-display" :args ["--json" "--mode" "display"]}
   {:name "json-tz-tokyo" :args ["--json"] :timezone "Asia/Tokyo"}])

(defn real-dates
  "`--until yesterday` keeps files an agent is writing right now out of every
  run, so two runs a moment apart read the same input."
  [^java.time.LocalDate today]
  (let [fmt #(.format ^java.time.LocalDate % java.time.format.DateTimeFormatter/BASIC_ISO_DATE)]
    {:until (fmt (.minusDays today 1)) :window-since (fmt (.minusDays today 30))}))

(defn command-plan
  "Every command line both binaries run. `dates` is {:since :until} for a
  fixture run and (real-dates today) for a real one."
  [{:keys [real]} dates]
  (for [command report-commands
        {:keys [name args window? timezone] :or {timezone "UTC"}} output-variants]
    {:id (str (str/replace command " " "-") "--" name)
     :json? (boolean (some #{"--json"} args))
     :args (vec (concat (str/split command #" ") ["--offline" "--timezone" timezone] args
                        (cond
                          (and real window?) ["--since" (:window-since dates) "--until" (:until dates)]
                          real ["--until" (:until dates)]
                          window? ["--since" (:since dates) "--until" (:until dates)])))}))

;; LOG_LEVEL=0 would hide box titles and warnings, which are part of the output under comparison.
(def output-vars (dissoc (:vars (local-logs-env)) "LOG_LEVEL"))

(defn hermetic-env [fixture-dir empty-home]
  {:clean? true
   :vars (merge (array-map "HOME" empty-home
                           "XDG_CACHE_HOME" (path empty-home ".cache")
                           "XDG_CONFIG_HOME" (path empty-home ".config")
                           "XDG_DATA_HOME" (path empty-home ".local" "share"))
                (when fixture-dir
                  (into (array-map)
                        (for [[agent variable] agent-env-vars
                              :let [dir (path fixture-dir agent)]
                              :when (fs/directory? dir)]
                          [variable dir])))
                output-vars)})

(defn real-env [] {:clean? false :vars output-vars})

(defn process-env [{:keys [clean? vars]} inherited]
  (if clean? (into {} vars) (merge (dissoc (into {} inherited) "LOG_LEVEL") vars)))

(defn run-captured!
  "Runs one binary and keeps stdout and stderr as files, so they are compared
  as bytes and never decoded."
  [bin args env dir prefix cwd]
  (let [out (path dir (str prefix ".stdout")) err (path dir (str prefix ".stderr"))
        result @(process/process (into [bin] args)
                                 (cond-> {:env (process-env env (System/getenv)) :in ""
                                          :out :write :out-file (fs/file out) :err :write :err-file (fs/file err)}
                                   cwd (assoc :dir cwd)))]
    {:exit (:exit result) :out-file out :err-file err
     :out (fs/read-all-bytes out) :err (fs/read-all-bytes err)}))

(defn differing-streams [a b allow-stderr-diff]
  (cond-> []
    (not= (:exit a) (:exit b)) (conj :exit)
    (not (java.util.Arrays/equals ^bytes (:out a) ^bytes (:out b))) (conj :stdout)
    (and (not allow-stderr-diff) (not (java.util.Arrays/equals ^bytes (:err a) ^bytes (:err b)))) (conj :stderr)))

(defn- first-differing
  "The first run that differs from `reference`, with the streams that differ."
  [reference runs allow-stderr-diff]
  (first (for [run runs
               :let [streams (differing-streams reference run allow-stderr-diff)]
               :when (seq streams)]
           {:run run :streams streams})))

(defn classify
  "Takes every run of each binary. A base that disagrees with itself cannot
  tell the refs apart, so that command is unstable rather than different. A
  head that disagrees with a reproducible base is different, even when only
  some of its runs do: nondeterminism the head introduced is a change.
  Matching failures cannot establish report equivalence."
  [{:keys [base head]} allow-stderr-diff]
  (let [reference (first base)
        verdict (fn [status found] {:status status :streams (:streams found) :pair [reference (:run found)]})]
    (if-let [unstable (first-differing reference (rest base) allow-stderr-diff)]
      (verdict :unstable unstable)
      (if-let [different (first-differing reference head allow-stderr-diff)]
        (verdict :different different)
        (if (zero? (:exit reference))
          {:status :identical :streams []}
          (verdict :failed {:run (first head) :streams [:exit]}))))))

(def confirmation-rounds
  "Extra runs of each binary once a first round disagrees, so a command that
  is only sometimes nondeterministic is not mistaken for a real difference."
  3)

(defn compare-command! [bins env output-dir cwd allow-stderr-diff {:keys [id args] :as command}]
  (let [dir (path output-dir id)
        _ (fs/create-dirs dir)
        run (fn [variant n] (assoc (run-captured! (get bins variant) args env dir (str (name variant) "-" n) cwd)
                                   :label (str (name variant) " run " n)))
        first-round {:base [(run :base 1)] :head [(run :head 1)]}
        first-round (-> first-round (update :base conj (run :base 2)) (update :head conj (run :head 2)))
        runs (if (#{:identical :failed} (:status (classify first-round allow-stderr-diff)))
               first-round
               (reduce (fn [runs n] (-> runs (update :base conj (run :base (+ 2 n))) (update :head conj (run :head (+ 2 n)))))
                       first-round (range 1 (inc confirmation-rounds))))
        verdict (classify runs allow-stderr-diff)]
    (when (= :identical (:status verdict)) (fs/delete-tree dir))
    (merge command verdict {:dir dir})))

(def diff-line-limit 40)

(defn- sorted-json! [file]
  (let [result (run-process ["jq" "-S" "." file])]
    (when (zero? (:exit result))
      (let [sorted (str file ".sorted.json")] (spit sorted (:out result)) sorted))))

(defn unified-diff
  "At most `diff-line-limit` lines; JSON goes through `jq -S` first so a
  reordered key does not bury the changed value. Raw bytes are shown when
  normalization would hide the entire difference."
  [left right json?]
  (let [[a b] (or (when json?
                    (let [a (sorted-json! left) b (sorted-json! right)] (when (and a b) [a b])))
                  [left right])
        difference (:out (run-process ["diff" "-u" a b]))
        difference (if (and (empty? difference) (not= [a b] [left right]))
                     (:out (run-process ["diff" "-u" left right]))
                     difference)
        lines (str/split-lines difference)]
    (str/join "\n" (cond-> (vec (take diff-line-limit lines))
                     (> (count lines) diff-line-limit)
                     (conj (format "... (%d more lines)" (- (count lines) diff-line-limit)))))))

(defn command-label [{:keys [args]}] (str/join " " (map command-text-arg args)))

(defn report-result! [{:keys [status streams pair json?] :as result}]
  (println (format "%-8s %s" (case status :identical "PASS" :different "FAIL" :unstable "UNSTABLE" :failed "ERROR") (command-label result)))
  (let [[a b] pair]
    (doseq [stream streams]
      (case stream
        :exit (println (format "  exit code: %s=%d, %s=%d" (:label a) (:exit a) (:label b) (:exit b)))
        (let [file (if (= :stdout stream) :out-file :err-file)]
          (println (format "  %s differs:\n    %s\n    %s" (name stream) (get a file) (get b file)))
          (println (unified-diff (get a file) (get b file) (and json? (= :stdout stream)))))))))

(defn summary-counts [results]
  (let [counts (frequencies (map :status results))]
    (array-map "Commands run" (count results) "Identical" (get counts :identical 0)
               "Different" (get counts :different 0) "Unstable" (get counts :unstable 0)
               "Failed" (get counts :failed 0))))

(defn parse-diff-cli [argv]
  (let [{:keys [args opts]} (cli/parse-args argv {:spec diff-spec :restrict true})]
    (when-not (:help opts)
      (when (not= 2 (count args))
        (fail! "Pass exactly two git refs or ccusage binary paths: <base> <head>" {:kind :invalid-cli}))
      (when (and (:real opts) (:fixture opts))
        (fail! "--real and --fixture are mutually exclusive" {:kind :invalid-cli}))
      (when (and (:fixture opts) (not (fs/directory? (:fixture opts))))
        (fail! (str "--fixture is not a directory: " (:fixture opts)) {:kind :invalid-cli}))
      (when (< (:jobs opts) 1)
        (fail! "--jobs must be at least 1" {:kind :invalid-cli})))
    (assoc opts :refs (vec args))))

(defn diff-help-text []
  (str "Usage: diff-refs.bb [options] <base> <head>\n\n"
       "Builds each git ref in release mode (or takes an existing binary path), runs every report\n"
       "command with both binaries under the same arguments and environment, and compares stdout,\n"
       "stderr, and exit code byte for byte. Variants cover JSON, tables, breakdowns, date windows,\n"
       "cost modes, and timezones. Agent-specific options and statusline hook payloads are covered\n"
       "by the golden tests. Each build runs twice, and both run three more times when\n"
       "anything disagrees: a command whose base output is not reproducible is reported as unstable\n"
       "instead of different. Exits 0 only when all commands succeed with identical output,\n"
       "1 on a difference, and 2 when comparisons are unstable or matching commands fail.\n\n"
       (cli/format-opts {:spec diff-spec})))

(defn- run-all! [jobs commands compare!]
  (let [pool (java.util.concurrent.Executors/newFixedThreadPool jobs)]
    (try
      (let [futures (mapv (fn [command] (.submit pool ^Callable (fn [] (compare! command)))) commands)]
        (mapv (fn [future] (doto (.get ^java.util.concurrent.Future future) report-result!)) futures))
      (finally (.shutdownNow pool)))))

(defn diff-main [& argv]
  (let [options (parse-diff-cli argv)]
    (if (:help options)
      (do (println (diff-help-text)) 0)
      (let [repo (repo-root)
            cache (expand (or (:cache-dir options) (path repo "rust" "target" "bench-history")))
            [base head] (resolve-variants repo cache (:refs options))
            bins {:base (:bin base) :head (:bin head)}
            real (:real options)
            empty-home (str (temp-dir "ccusage-diff-home."))
            output-dir (if (:output-dir options)
                         (doto (expand (:output-dir options)) fs/create-dirs)
                         (str (temp-dir "ccusage-diff-refs.")))
            env (if real (real-env) (hermetic-env (some-> (:fixture options) expand) empty-home))
            dates (if real
                    (real-dates (java.time.LocalDate/now java.time.ZoneOffset/UTC))
                    (select-keys options [:since :until]))
            commands (command-plan options dates)]
        (try
          (write-progress (format "comparing %d commands: base %s, head %s (%s)" (count commands) (:bin base) (:bin head)
                                  (cond real "real local logs" (:fixture options) (str "fixture " (:fixture options)) :else "empty environment")))
          (let [results (run-all! (:jobs options) commands
                                  #(compare-command! bins env output-dir (when-not real empty-home) (:allow-stderr-diff options) %))
                counts (summary-counts results)]
            (println)
            (println (table-md [counts]))
            (when-let [unstable (seq (filter #(= :unstable (:status %)) results))]
              (println "\nUnstable (base differs between runs of the same binary):")
              (doseq [result unstable] (println (str "  " (command-label result)))))
            (if (= (get counts "Identical") (count results))
              (when-not (:output-dir options) (fs/delete-tree output-dir))
              (println (str "\nSaved outputs: " output-dir)))
            (cond
              (pos? (get counts "Different")) 1
              (pos? (get counts "Unstable")) 2
              (pos? (get counts "Failed")) 2
              :else 0))
          (finally (fs/delete-tree empty-home)))))))
