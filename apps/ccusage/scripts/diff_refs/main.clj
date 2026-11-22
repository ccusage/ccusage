(ns diff-refs.main
  (:require [babashka.cli :as cli]
            [babashka.fs :as fs]
            [babashka.process :as process]
            [cheshire.core :as json]
            [clojure.string :as str]
            [compare-pr-performance.data :as data]
            [compare-pr-performance.history :as history]
            [compare-pr-performance.report :as report]
            [compare-pr-performance.system :as system])
  (:import [java.util.concurrent Callable]))

(def diff-spec
  {:real {:coerce :boolean :desc "Read the caller's real local agent logs instead of a hermetic environment"}
   :fixture {:ref "DIR" :desc "Golden fixture to reuse; exported there first when DIR is missing or empty (default: a fresh export)"}
   :allow-stderr-diff {:coerce :boolean :desc "Compare stdout and exit code only"}
   :since {:coerce :string :ref "YYYYMMDD" :desc "Start of the date-window variant (fixture mode; default: the golden window)"}
   :until {:coerce :string :ref "YYYYMMDD" :desc "End of the date-window variant (fixture mode; default: the golden window)"}
   :jobs {:coerce :long :default 1 :ref "N" :desc "Commands compared in parallel; above 1 some adapters stop being reproducible on real logs"}
   :output-dir {:ref "DIR" :desc "Where outputs of differing commands are kept (default: a private temp dir)"}
   :cache-dir {:ref "DIR" :desc "Built binaries and shared cargo target (default: rust/target/bench-history)"}
   :help {:alias :h :coerce :boolean}})

(defn help-commands
  [help]
  (->> (str/split-lines help)
       (drop-while #(not= "COMMANDS:" (str/trim %)))
       rest
       (take-while #(not (str/blank? %)))
       (map #(first (str/split (str/trim %) #"\s+")))))

(defn report-commands
  [bin]
  (let [help (fn [& command]
               (:out (system/run-checked (into [bin] (concat command ["--help"])) {}
                                  (str/join " " (concat [bin] command ["--help"])))))
        report? #(not= "statusline" %)
        commands (vec (mapcat (fn [command]
                                (if-let [reports (seq (help-commands (help command)))]
                                  (for [report reports :when (report? report)] (str command " " report))
                                  (when (report? command) [command])))
                              (help-commands (help))))]
    (when (empty? commands)
      (data/fail! (str bin " --help lists no report commands") {:kind :invalid-help}))
    commands))

(def output-variants
  [{:name "json" :args ["--json"]}
   {:name "table" :args []}
   {:name "json-breakdown" :args ["--json" "--breakdown"]}
   {:name "json-window" :args ["--json"] :window? true}
   {:name "json-mode-calculate" :args ["--json" "--mode" "calculate"]}
   {:name "json-mode-display" :args ["--json" "--mode" "display"]}
   {:name "json-tz-tokyo" :args ["--json"] :timezone "Asia/Tokyo"}])

(defn real-dates
  [^java.time.LocalDate today]
  (let [fmt #(.format ^java.time.LocalDate % java.time.format.DateTimeFormatter/BASIC_ISO_DATE)]
    {:until (fmt (.minusDays today 1)) :window-since (fmt (.minusDays today 30))}))

(defn command-plan
  [{:keys [real]} dates commands]
  (for [command commands
        {:keys [name args window? timezone] :or {timezone "UTC"}} output-variants]
    {:id (str (str/replace command " " "-") "--" name)
     :json? (boolean (some #{"--json"} args))
     :args (vec (concat (str/split command #" ") ["--offline" "--timezone" timezone] args
                        (cond
                          (and real window?) ["--since" (:window-since dates) "--until" (:until dates)]
                          real ["--until" (:until dates)]
                          window? ["--since" (:since dates) "--until" (:until dates)])))}))

(def fixture-manifest
  "golden-env.json")

(defn export-golden-fixture!
  [repo dir]
  (system/write-progress (str "exporting the golden fixture to " dir))
  (system/run-checked ["cargo" "test" "--manifest-path" (system/path repo "rust" "Cargo.toml") "--package" "ccusage"
                "--test" "golden_matrix" "--" "--ignored" "--exact" "export_golden_fixture"]
               {:env {"CCUSAGE_GOLDEN_FIXTURE_DIR" dir}} "golden fixture export"))

(defn- manifest-problems
  [manifest]
  (let [env (get manifest "env")
        seeded (get manifest "seeded")
        bound? #(and (string? %) (re-matches #"\d{8}" %))]
    (if-not (map? manifest)
      ["it is not a JSON object"]
      (cond-> []
        (not (and (string? (get manifest "cwd")) (fs/directory? (get manifest "cwd"))))
        (conj "cwd is not an existing directory")
        (not (and (map? env) (seq env) (every? string? (vals env))))
        (conj "env is not a non-empty map of strings")
        (not (and (bound? (get-in manifest ["window" "since"])) (bound? (get-in manifest ["window" "until"]))))
        (conj "window does not hold YYYYMMDD since and until bounds")
        (not (and (sequential? seeded) (seq seeded)))
        (conj "seeded lists no store variables")
        (sequential? seeded)
        (into (for [name seeded
                    :let [stores (get env name)]
                    :when (not (and (string? stores)
                                    (some #(fs/exists? (str/trim %)) (str/split stores #","))))]
                (str "seeded variable " name " names no existing store")))))))

(defn read-golden-fixture [dir]
  (let [manifest (system/path dir fixture-manifest)]
    (when-not (fs/regular-file? manifest)
      (data/fail! (str dir " is not a golden fixture: " fixture-manifest " is missing") {:kind :invalid-fixture}))
    (let [parsed (try (json/parse-string (slurp manifest))
                      (catch Exception error {::unparsable (ex-message error)}))
          problems (if-let [error (::unparsable parsed)]
                     [(str fixture-manifest " is not JSON: " error)]
                     (manifest-problems parsed))]
      (when (seq problems)
        (data/fail! (str dir " is not a usable golden fixture: " (str/join "; " problems)
                    ". Pass an empty --fixture directory to export a fresh one.")
               {:kind :invalid-fixture}))
      parsed)))

(defn prepare-golden-fixture!
  [repo dir]
  (when (or (not (fs/exists? dir)) (and (fs/directory? dir) (empty? (fs/list-dir dir))))
    (fs/create-dirs dir)
    (export-golden-fixture! repo dir))
  (read-golden-fixture dir))

(def test-clock-var "CCUSAGE_TEST_NOW")

(def test-clock-features ["ccusage/test-clock" "ccusage-core/test-clock"])

(defn declares-test-clock? [cargo-toml]
  (boolean (and cargo-toml (re-find #"(?m)^test-clock\s*=" cargo-toml))))

(defn clock-features
  [repo commit]
  (or (some (fn [feature]
              (let [package (first (str/split feature #"/"))
                    manifest (system/run-process ["git" "show" (str commit ":rust/crates/" package "/Cargo.toml")] {:dir repo})]
                (when (and (zero? (:exit manifest)) (declares-test-clock? (:out manifest)))
                  [feature])))
            test-clock-features)
      []))

(defn pins-clock? [variant] (boolean (some (set test-clock-features) (:features variant))))

(defn golden-env
  [manifest pinned-clock?]
  {:clean? true
   :vars (merge (into (sorted-map) (cond-> (dissoc (get manifest "env") "LOG_LEVEL")
                                     (not pinned-clock?) (dissoc test-clock-var)))
                system/report-vars)})

(defn real-env [] {:clean? false :vars system/report-vars})

(defn process-env [{:keys [clean? vars]} inherited]
  (if clean? (into {} vars) (merge (dissoc (into {} inherited) "LOG_LEVEL" test-clock-var) vars)))

(defn run-captured!
  [bin args env dir prefix cwd]
  (let [out (system/path dir (str prefix ".stdout")) err (system/path dir (str prefix ".stderr"))
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
  [reference runs allow-stderr-diff]
  (first (for [run runs
               :let [streams (differing-streams reference run allow-stderr-diff)]
               :when (seq streams)]
           {:run run :streams streams})))

(defn classify
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
  (let [dir (system/path output-dir id)
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
  (let [result (system/run-process ["jq" "-S" "." file])]
    (when (zero? (:exit result))
      (let [sorted (str file ".sorted.json")] (spit sorted (:out result)) sorted))))

(defn unified-diff
  [left right json?]
  (let [[a b] (or (when json?
                    (let [a (sorted-json! left) b (sorted-json! right)] (when (and a b) [a b])))
                  [left right])
        difference (:out (system/run-process ["diff" "-u" a b]))
        difference (if (and (empty? difference) (not= [a b] [left right]))
                     (:out (system/run-process ["diff" "-u" left right]))
                     difference)
        lines (str/split-lines difference)]
    (str/join "\n" (cond-> (vec (take diff-line-limit lines))
                     (> (count lines) diff-line-limit)
                     (conj (format "... (%d more lines)" (- (count lines) diff-line-limit)))))))

(defn command-label [{:keys [args]}] (str/join " " (map data/command-text-arg args)))

(defn report-result! [{:keys [status streams pair json? error] :as result}]
  (println (format "%-8s %s" (case status :identical "PASS" :different "FAIL" :unstable "UNSTABLE" :failed "ERROR") (command-label result)))
  (when error (println (str "  comparison could not run (" error ")")))
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
        (data/fail! "Pass exactly two git refs or ccusage binary paths: <base> <head>" {:kind :invalid-cli}))
      (when (and (:real opts) (:fixture opts))
        (data/fail! "--real and --fixture are mutually exclusive" {:kind :invalid-cli}))
      (when (and (:fixture opts) (fs/exists? (:fixture opts)) (not (fs/directory? (:fixture opts))))
        (data/fail! (str "--fixture is not a directory: " (:fixture opts)) {:kind :invalid-cli}))
      (when (< (:jobs opts) 1)
        (data/fail! "--jobs must be at least 1" {:kind :invalid-cli})))
    (assoc opts :refs (vec args))))

(defn diff-help-text []
  (str "Usage: diff-refs.bb [options] <base> <head>\n\n"
       "Builds each git ref in release mode (or takes an existing binary path), runs the shared report\n"
       "commands listed in the head binary's help with both binaries under the same arguments and\n"
       "environment, and compares stdout, stderr, and exit code byte for byte. Without --real, both\n"
       "binaries read the golden matrix seeds of this checkout, exported by `cargo test`, at the\n"
       "golden clock: refs are built with the dev-only test-clock feature when they\n"
       "declare it, and when either side cannot honor the clock (an older ref or a binary path)\n"
       "both run under the wall clock instead, with a notice. Variants\n"
       "cover JSON, tables, breakdowns, date windows, cost modes, and timezones. Agent-specific\n"
       "options, including statusline, are covered by the golden tests. Each command runs\n"
       "twice per binary, then three more times per binary when the results disagree: a command\n"
       "whose base output is not reproducible is reported as unstable instead of different. Exits 0\n"
       "only when all commands succeed with identical output, 1 on a confirmed difference, and 2\n"
       "when a comparison cannot complete successfully.\n\n"
       (cli/format-opts {:spec diff-spec})))

(defn- run-all! [jobs commands compare!]
  (let [pool (java.util.concurrent.Executors/newFixedThreadPool jobs)]
    (try
      (let [futures (mapv (fn [command]
                            (.submit pool ^Callable
                                     (fn []
                                       (try
                                         (compare! command)
                                         (catch Exception error
                                           (assoc command :status :failed :streams []
                                                  :error (str (.getSimpleName (class error)) ": " (ex-message error))))))))
                          commands)]
        (mapv (fn [future] (doto (.get ^java.util.concurrent.Future future) report-result!)) futures))
      (finally (.shutdownNow pool)))))

(defn diff-main [& argv]
  (let [options (parse-diff-cli argv)]
    (if (:help options)
      (do (println (diff-help-text)) 0)
      (let [repo (history/repo-root)
            cache (system/expand (or (:cache-dir options) (system/path repo "rust" "target" "bench-history")))
            [base head :as variants] (history/resolve-variants repo cache (:refs options) (partial clock-features repo))
            bins {:base (:bin base) :head (:bin head)}
            real (:real options)
            fixture-dir (when-not real
                          (if (:fixture options) (system/expand (:fixture options)) (str (system/temp-dir "ccusage-diff-fixture."))))
            output-dir (if (:output-dir options)
                         (doto (system/expand (:output-dir options)) fs/create-dirs)
                         (str (system/temp-dir "ccusage-diff-refs.")))]
        (try
          (let [manifest (when fixture-dir (prepare-golden-fixture! repo fixture-dir))
                unpinned (remove pins-clock? variants)
                _ (when (and manifest (contains? (get manifest "env") test-clock-var) (seq unpinned))
                    (system/write-progress (str (str/join ", " (map :label unpinned))
                                         " cannot honor " test-clock-var " (an existing binary, or a ref built without "
                                         "a test-clock feature), so both binaries run under the wall clock;"
                                         " clock-relative reports are compared at the current time")))
                env (if real (real-env) (golden-env manifest (empty? unpinned)))
                dates (if real
                        (real-dates (java.time.LocalDate/now java.time.ZoneOffset/UTC))
                        (merge (update-keys (get manifest "window") keyword)
                               (into {} (filter val (select-keys options [:since :until])))))
                commands (command-plan options dates (report-commands (:head bins)))]
            (system/write-progress (format "comparing %d commands: base %s, head %s (%s)" (count commands) (:bin base) (:bin head)
                                    (if real "real local logs" (str "golden fixture " fixture-dir))))
            (let [results (run-all! (:jobs options) commands
                                    #(compare-command! bins env output-dir (get manifest "cwd") (:allow-stderr-diff options) %))
                  counts (summary-counts results)]
              (println)
              (println (report/table-md [counts]))
              (when-let [unstable (seq (filter #(= :unstable (:status %)) results))]
                (println "\nUnstable (base differs between runs of the same binary):")
                (doseq [result unstable] (println (str "  " (command-label result)))))
              (if (= (get counts "Identical") (count results))
                (when-not (:output-dir options) (fs/delete-tree output-dir))
                (println (str "\nSaved outputs: " output-dir)))
              (cond
                (pos? (get counts "Unstable")) 2
                (pos? (get counts "Failed")) 2
                (pos? (get counts "Different")) 1
                :else 0)))
          (finally
            (when (and fixture-dir (not (:fixture options))) (fs/delete-tree fixture-dir))))))))
