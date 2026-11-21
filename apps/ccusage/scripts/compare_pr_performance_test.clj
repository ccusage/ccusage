#!/usr/bin/env nix
#! nix shell --inputs-from ../../.. nixpkgs#babashka-unwrapped nixpkgs#bash nixpkgs#time --command bb

(ns compare-pr-performance-test
  (:require [babashka.classpath :as cp]
            [babashka.fs :as fs]
            [babashka.process]
            [cheshire.core]
            [clojure.string]
            [clojure.test :refer [deftest is run-tests testing]]))

(def script-dir (fs/parent (fs/absolutize *file*)))
(cp/add-classpath (str script-dir))

(require '[compare-pr-performance.benchmark :as benchmark]
         '[compare-pr-performance.data :as data]
         '[compare-pr-performance.history :as history]
         '[compare-pr-performance.main :as main]
         '[compare-pr-performance.report :as report]
         '[compare-pr-performance.system :as system])

(def required-args ["--base-bin" "base" "--head-bin" "head" "--fixture-dir" "claude" "--codex-fixture-dir" "codex"])

(deftest entrypoints-from-another-directory
  (let [dir (fs/create-temp-dir {:prefix "ccusage-entrypoints."})
        bb (str (fs/which "bb"))]
    (try
      (doseq [entry ["compare-pr-performance" "bench-history" "diff-refs"]]
        (let [result (system/run-process [bb (system/path script-dir (str entry ".bb")) "--help"] {:dir dir})]
          (is (zero? (:exit result)) (:err result))
          (is (clojure.string/starts-with? (:out result) (str "Usage: " entry ".bb ")))))
      (finally (fs/delete-tree dir)))))

(deftest cli-parsing
  (testing "defaults"
    (is (= {:runs 11 :warmup 2 :threshold 0.1 :wall-threshold 0.08}
           (select-keys (data/parse-cli required-args) [:runs :warmup :threshold :wall-threshold])))
    (is (thrown-with-msg? clojure.lang.ExceptionInfo #"--wall-threshold must be between 0 and 1"
                          (data/parse-cli (into required-args ["--wall-threshold" "0"])))))
  (testing "too few rounds to judge a regression"
    (is (thrown-with-msg? clojure.lang.ExceptionInfo #"--runs must be at least 3"
                          (data/parse-cli (conj required-args "--runs" "2")))))
  (testing "missing binary"
    (is (thrown-with-msg? clojure.lang.ExceptionInfo #"--head-bin is required"
                          (data/parse-cli ["--base-bin" "base" "--fixture-dir" "claude" "--codex-fixture-dir" "codex"]))))
  (testing "history takes refs as positional arguments"
    (is (= ["v20.0.20" "main"] (:refs (history/parse-history-cli ["--real" "v20.0.20" "main"]))))
    (is (true? (:real (history/parse-history-cli ["--real" "main"]))))
    (is (thrown-with-msg? clojure.lang.ExceptionInfo #"at least one git ref"
                          (history/parse-history-cli []))))
  (testing "history labels stay unique"
    (is (thrown-with-msg? clojure.lang.ExceptionInfo #"may appear once: main"
                          (history/parse-history-cli ["main" "v20.0.20" "main"])))
    (is (= ["/a/ccusage" "/b/ccusage"] (:refs (history/parse-history-cli ["/a/ccusage" "/b/ccusage"])))))
  (testing "a custom Claude fixture needs its Codex fixture"
    (is (thrown-with-msg? clojure.lang.ExceptionInfo #"--fixture-dir and --codex-fixture-dir together"
                          (history/parse-history-cli ["--fixture-dir" "c" "main"])))
    (is (thrown-with-msg? clojure.lang.ExceptionInfo #"--fixture-dir and --codex-fixture-dir together"
                          (history/parse-history-cli ["--codex-fixture-dir" "x" "main"]))))
  (testing "a custom Grok fixture joins a custom Claude and Codex pair"
    (is (= "g" (:grok-fixture-dir (history/parse-history-cli ["--fixture-dir" "c" "--codex-fixture-dir" "x" "--grok-fixture-dir" "g" "main"]))))
    (is (thrown-with-msg? clojure.lang.ExceptionInfo #"--grok-fixture-dir with --fixture-dir"
                          (history/parse-history-cli ["--grok-fixture-dir" "g" "main"]))))
  (testing "history rejects too few rounds instead of measuring them"
    (is (thrown-with-msg? clojure.lang.ExceptionInfo #"--runs must be at least 3"
                          (history/parse-history-cli ["--runs" "2" "main"])))
    (is (= 3 (:runs (history/parse-history-cli ["--runs" "3" "main"]))))))

(deftest formatting
  (is (= "999ms" (data/format-duration 999)))
  (is (= "1.00s" (data/format-duration 1000)))
  (is (= "1.50 KiB" (data/format-size 1536)))
  (is (= "0123456789ab" (data/format-sha "0123456789abcdef")))
  (is (= "1.00 (0.90–1.20)" (report/format-ratio-spread {:median 1.0 :min 0.9 :max 1.2}))))

(deftest command-rendering
  (is (= "env CLAUDE_CONFIG_DIR=/tmp/a \"CODEX_HOME=/tmp/codex data\" /tmp/ccusage claude daily --offline --json"
         (data/benchmark-command-text
          {:vars {"CLAUDE_CONFIG_DIR" "/tmp/a" "CODEX_HOME" "/tmp/codex data"}}
          (benchmark/command-argv "/tmp/ccusage" "claude daily"))))
  (testing "fixture runs start from an empty environment"
    (is (= "env -i CLAUDE_CONFIG_DIR=/f/c CODEX_HOME=/f/x HOME=/e COLUMNS=200 LOG_LEVEL=0 NO_COLOR=1 TZ=UTC /b daily --offline --json"
           (data/benchmark-command-text (system/fixture-env "/f/c" "/f/x" nil "/e") (benchmark/command-argv "/b" "daily")))))
  (testing "a Grok fixture is pointed at through GROK_HOME"
    (is (= "env -i CLAUDE_CONFIG_DIR=/f/c CODEX_HOME=/f/x GROK_HOME=/f/g HOME=/e COLUMNS=200 LOG_LEVEL=0 NO_COLOR=1 TZ=UTC /b daily --offline --json"
           (data/benchmark-command-text (system/fixture-env "/f/c" "/f/x" "/f/g" "/e") (benchmark/command-argv "/b" "daily")))))
  (testing "a Claude-only shape fixture leaves CODEX_HOME unset"
    (is (not (.contains (data/benchmark-command-text (system/fixture-env "/f/c" nil nil "/e") (benchmark/command-argv "/b" "daily"))
                        "CODEX_HOME"))))
  (testing "a shape fixture is pointed at through its own agent's variable"
    (let [shape (first (filter #(= :opencode (:fixture %)) data/shape-fixtures))]
      (is (= "env -i OPENCODE_DATA_DIR=/f/o HOME=/e COLUMNS=200 LOG_LEVEL=0 NO_COLOR=1 TZ=UTC /b opencode daily --offline --json"
             (data/benchmark-command-text (system/shape-env shape "/f/o" "/e") (benchmark/command-argv "/b" "opencode daily"))))
      (is (some #{{:fixture :opencode :command "opencode daily"}} data/shape-commands))))
  (testing "a shape fixture without its own variable is a Claude fixture"
    (is (= "env -i CLAUDE_CONFIG_DIR=/f/c HOME=/e COLUMNS=200 LOG_LEVEL=0 NO_COLOR=1 TZ=UTC /b daily --offline --json"
           (data/benchmark-command-text (system/shape-env {:fixture :small} "/f/c" "/e") (benchmark/command-argv "/b" "daily"))))))

(deftest statistics
  (is (= 4 (data/median [9 1 2 4 7])))
  (is (= 3.0 (data/median [9 1 2 4])))
  (is (= {:median 3.0 :min 1 :max 9 :samples 4} (data/summarize [9 1 2 4])))
  (is (= ["b" "c" "a"] (benchmark/rotate ["a" "b" "c"] 1)))
  (is (= ["a" "b" "c"] (benchmark/rotate ["a" "b" "c"] 3))))

(deftest regression-classification
  (testing "a consistent slowdown past the threshold"
    (is (= :regression (data/classify-ratios [1.5 1.6 1.4 1.55 1.5 1.45 1.6 1.5 1.52 1.48 1.5] 0.1))))
  (testing "a consistent speedup"
    (is (= :improvement (data/classify-ratios [0.6 0.62 0.58 0.61 0.6] 0.1))))
  (testing "noise around 1.0 stays unchanged"
    (is (= :unchanged (data/classify-ratios [1.03 0.97 1.05 0.99 1.02 0.98 1.04 1.01 0.96 1.0 1.02] 0.1))))
  (testing "one outlier round cannot flag a regression"
    (is (= :unchanged (data/classify-ratios [1.0 1.02 0.98 1.01 2.5 0.99 1.0 1.03 0.97 1.0 1.01] 0.1))))
  (testing "a large median without agreement across rounds stays unchanged"
    (is (= :unchanged (data/classify-ratios [1.3 1.3 1.3 0.9 0.9 1.3 0.9] 0.1)))))

(defn round [base-cpu head-cpu]
  {"base" {:wall (* 0.2 base-cpu) :cpu base-cpu :rss 100 :instructions nil}
   "PR" {:wall (* 0.2 head-cpu) :cpu head-cpu :rss 100 :instructions nil}})

(def limits {:threshold 0.1 :wall-threshold 0.05})

(defn timed-round [[base-wall head-wall] [base-cpu head-cpu]]
  {"base" {:wall base-wall :cpu base-cpu :rss 100 :instructions nil}
   "PR" {:wall head-wall :cpu head-cpu :rss 100 :instructions nil}})

(deftest paired-comparison
  (let [rounds [(round 1000 1500) (round 1100 1600) (round 900 1400) (round 1000 1550) (round 1050 1500)]
        comparison (benchmark/compare-variants rounds "base" "PR" limits)]
    (is (= :regression (get-in comparison [:cpu :verdict])))
    (is (= :regression (get-in comparison [:wall :verdict])))
    (is (= :unchanged (get-in comparison [:rss :verdict])))
    (is (nil? (:instructions comparison)))
    (is (= "⚠️ slower, more CPU" (report/comparison-verdict comparison)))
    (is (= 1000 (get-in (benchmark/variant-summary rounds "base") [:cpu :median])))))

(deftest wall-time-is-judged
  (testing "a single-thread rewrite like #2407's state: slower wall, less CPU"
    (let [rounds (mapv #(timed-round [280 (+ 380 %)] [750 370]) [0 4 -3 6 2 -1 5])
          comparison (benchmark/compare-variants rounds "base" "PR" limits)]
      (is (= :regression (get-in comparison [:wall :verdict])))
      (is (= :improvement (get-in comparison [:cpu :verdict])))
      (is (= "⚠️ slower, less CPU" (report/comparison-verdict comparison)))
      (is (:regression? (report/overall-verdict [{:command "opencode daily" :comparison comparison}] limits)))))
  (testing "wall time uses its own threshold"
    (let [rounds (mapv (fn [_] (timed-round [1000 1040] [1000 1000])) (range 5))]
      (is (= :unchanged (get-in (benchmark/compare-variants rounds "base" "PR" limits) [:wall :verdict])))
      (is (= :regression (get-in (benchmark/compare-variants rounds "base" "PR" (assoc limits :wall-threshold 0.03)) [:wall :verdict])))))
  (testing "a faster wall with more CPU, like #2412, reads as both"
    (let [rounds (mapv (fn [_] (timed-round [380 280] [370 750])) (range 5))]
      (is (= "⚠️ faster, more CPU" (report/comparison-verdict (benchmark/compare-variants rounds "base" "PR" limits)))))))

(defn with-hyperfine-export [content f]
  (let [dir (babashka.fs/create-temp-dir {:prefix "ccusage-hyperfine-test."})
        output (system/path dir "hyperfine.json")]
    (try
      (spit output content)
      (f output)
      (finally
        (babashka.fs/delete-tree dir)))))

(defn hyperfine-result [name instructions]
  {:name name :command "x"
   :measurements [{:instructions {:value instructions :unit "count"} :exit_code 0}]})

(defn hyperfine-export [results]
  (cheshire.core/generate-string {:schema_version 2 :results results}))

(deftest hyperfine-instructions
  (with-hyperfine-export
    (hyperfine-export [(hyperfine-result "PR" 200.0) (hyperfine-result "base" 100.0)])
    (fn [output]
      (is (= {"base" 100.0 "PR" 200.0} (benchmark/read-hyperfine-instructions output ["base" "PR"])))))
  (testing "a runner without hardware counters reports no instructions"
    (with-hyperfine-export
      (hyperfine-export [{:name "base" :command "x" :measurements [{:exit_code 0}]}])
      (fn [output]
        (is (= {"base" nil} (benchmark/read-hyperfine-instructions output ["base"]))))))
  (with-hyperfine-export
    "not json"
    (fn [output]
      (is (thrown-with-msg? clojure.lang.ExceptionInfo #"Malformed hyperfine output"
                            (benchmark/read-hyperfine-instructions output ["base"])))))
  (with-hyperfine-export
    (cheshire.core/generate-string {:results []})
    (fn [output]
      (is (thrown-with-msg? clojure.lang.ExceptionInfo #"schema version"
                            (benchmark/read-hyperfine-instructions output ["base"])))))
  (with-hyperfine-export
    (hyperfine-export [(hyperfine-result "base" 1.0)])
    (fn [output]
      (is (thrown-with-msg? clojure.lang.ExceptionInfo #"0 results named PR"
                            (benchmark/read-hyperfine-instructions output ["base" "PR"])))))
  (with-hyperfine-export
    (hyperfine-export [(hyperfine-result "base" 1.0) (hyperfine-result "base" 2.0)])
    (fn [output]
      (is (thrown-with-msg? clojure.lang.ExceptionInfo #"2 results named base"
                            (benchmark/read-hyperfine-instructions output ["base"]))))))

(deftest instruction-pass-tolerates-missing-counters
  (let [argv (atom nil)]
    (with-redefs [system/run-checked (fn [a _ _]
                                       (reset! argv a)
                                       (spit (second (drop-while #(not= "--export-json" %) a))
                                             (hyperfine-export [{:name "base" :command "x" :measurements [{:exit_code 0}]}
                                                                {:name "PR" :command "x" :measurements [{:exit_code 0}]}]))
                                       {:exit 0})]
      (is (= {"base" nil "PR" nil}
             (benchmark/instruction-counts [{:label "base" :bin "/b"} {:label "PR" :bin "/p"}] "daily" {:vars {}} "t")))
      (testing "hyperfine is asked for every metric, so it skips counters the runner lacks instead of failing"
        (is (= "all" (second (drop-while #(not= "--metrics" %) @argv))))))))

(deftest timed-runs
  (testing "wall, CPU, and peak RSS come from the process itself, not from hyperfine"
    (let [{:keys [wall cpu rss]} (benchmark/timed-run ["sleep" "0.2"] "sleep")]
      (is (<= 200 wall 1000) (str "wall " wall))
      (is (< cpu 100) (str "cpu " cpu))
      (is (pos? rss))))
  (testing "the timing line is parsed in milliseconds and the RSS in bytes"
    (is (= {:wall 281.0 :cpu 748.0 :rss (* 57280 1024)} (benchmark/parse-timing "0.281000 0.515000 0.233000\n" "57280\n"))))
  (testing "a time that rounds up to a whole second keeps its digits"
    (let [{:keys [wall]} (benchmark/timed-run ["sleep" "0.9999"] "sleep")]
      (is (< 999 wall 1500) (str "wall " wall))))
  (testing "a failing command stops the benchmark with its stderr"
    (is (thrown-with-msg? clojure.lang.ExceptionInfo #"(?s)broken.*boom"
                          (benchmark/timed-run ["bash" "-c" "echo boom >&2; exit 3"] "broken")))))

(deftest rounds-carry-one-instruction-count
  (let [calls (atom [])]
    (with-redefs [benchmark/timed-run (fn [argv _] (swap! calls conj (some #{"/b" "/p"} argv)) {:wall 10.0 :cpu 20.0 :rss 1024})
                  benchmark/instruction-counts (fn [variants _ _ _] (zipmap (map :label variants) [100.0 150.0]))]
      (let [{:keys [rounds]} (benchmark/measure-command [{:label "base" :bin "/b"} {:label "PR" :bin "/p"}] "daily" {:vars {}}
                                                        {:runs 3 :warmup 1 :title "t"})]
        (is (= 3 (count rounds)))
        (is (every? #(= 150.0 (get-in % ["PR" :instructions])) rounds))
        (is (= 1.5 (get-in (benchmark/compare-variants rounds "base" "PR" limits) [:instructions :median])))
        (testing "the one instruction count renders as a single ratio, not a spread"
          (let [comparison (benchmark/compare-variants rounds "base" "PR" limits)
                markdown (report/table-md (report/comparison-rows [{:command "daily" :base (benchmark/variant-summary rounds "base")
                                                                    :head (benchmark/variant-summary rounds "PR") :comparison comparison}]))]
            (is (.contains markdown "| 1.50 |"))
            (is (not (.contains markdown "1.50 (1.50–1.50)")))))
        (testing "every warmup and measured round runs both binaries, alternating which goes first"
          (is (= ["/b" "/p" "/b" "/p" "/p" "/b" "/b" "/p"] @calls)))))))

(deftest report-rendering
  (let [rounds [(round 1000 600) (round 1000 620) (round 1000 610)]
        result {:command "claude daily" :base (benchmark/variant-summary rounds "base") :head (benchmark/variant-summary rounds "PR")
                :comparison (benchmark/compare-variants rounds "base" "PR" limits)}
        context {:base-sha (apply str (repeat 40 "1")) :head-sha (apply str (repeat 40 "2")) :threshold 0.1 :wall-threshold 0.05 :runs 3 :warmup 1
                 :fixture-stats {:bytes 1073741824 :files 10} :codex-fixture-stats {:bytes 1048576 :files 2}}
        markdown (report/render-report context [result] [] {:base 2048 :head 1024})
        header (slurp (system/path (.getParent (babashka.fs/file *file*)) "testdata" "perf-comment-header.txt"))]
    (testing "the PR comment upsert reads its marker from this header"
      (is (.startsWith markdown header)))
    (testing "a report without a head commit carries no comment marker"
      (is (.startsWith (report/render-report (dissoc context :head-sha) [result] [] {:base 2048 :head 1024})
                       "## ccusage performance comparison\n")))
    (is (.contains markdown "**Verdict: ✅ improved** in `claude daily` (faster, less CPU)"))
    (testing "the method line says wall time is judged, with its own threshold"
      (is (.contains markdown "median wall-time ratio moves past 5%"))
      (is (not (.contains markdown "not judged"))))
    (is (.contains markdown "| Wall ratio |"))
    (is (.contains markdown "| `claude daily` | 1.00s | 610ms | 0.61 (0.60–0.62) |"))
    (is (.contains markdown "| release binary | 2.00 KiB | 1.00 KiB | -1.00 KiB |"))
    (is (.contains markdown "Claude logs (1.00 GiB on disk, 10 files) and Codex logs"))
    (is (.contains (report/fixture-line (assoc context :grok-fixture-stats {:bytes 2097152 :files 3}))
                   "Claude logs (1.00 GiB on disk, 10 files), Codex logs (1.00 MiB on disk, 2 files, mostly zstd-compressed), and Grok logs (2.00 MiB on disk, 3 files) shaped"))
    (testing "without shape results there is no second table"
      (is (not (.contains markdown "### Targeted shapes"))))
    (testing "shape results get their own table, named by fixture"
      (let [shapes [{:label "main"} {:label "tiny" :desc "startup dominates" :stats {:bytes 4194304 :files 5}}]
            slower {:command "daily" :fixture-label "tiny" :base (benchmark/variant-summary rounds "PR")
                    :head (benchmark/variant-summary rounds "base")
                    :comparison (benchmark/compare-variants rounds "PR" "base" limits)}
            markdown (report/render-report (assoc context :shapes shapes) [result] [slower] {:base 2048 :head 1024})]
        (is (.contains markdown "### Targeted shapes"))
        (is (.contains markdown "- **tiny** (4.00 MiB on disk, 5 files): startup dominates."))
        (is (.contains markdown "- **main**: the fixture above."))
        (is (.contains markdown "| Fixture | Command | CPU base |"))
        (is (.contains markdown "| tiny | `daily` | 610ms | 1.00s |"))
        (is (.contains markdown "**Verdict: ⚠️ possible regression** in `daily` on tiny (slower, more CPU)"))))))

(deftest since-cut-off-follows-the-fixture
  (let [dir (babashka.fs/create-temp-dir {:prefix "ccusage-since."})
        day (fn [d] (java.nio.file.attribute.FileTime/fromMillis (+ (* d 86400000) 1780300000000)))]
    (try
      (doseq [[i d] (map-indexed vector [0 3 7 12 20])]
        (let [file (babashka.fs/path dir "projects" (str "p" i) "s.jsonl")]
          (babashka.fs/create-dirs (babashka.fs/parent file))
          (spit (str file) "{}\n")
          (babashka.fs/set-last-modified-time file (day d))))
      (let [command (system/resolve-command (str "claude daily --since " data/median-mtime) dir)
            since (last (clojure.string/split command #" "))
            pruned (count (filter #(neg? (compare (.format (java.time.format.DateTimeFormatter/ofPattern "yyyyMMdd")
                                                           (java.time.LocalDate/ofInstant (.toInstant (babashka.fs/last-modified-time %)) java.time.ZoneOffset/UTC))
                                                  since))
                                  (babashka.fs/glob dir "**.jsonl")))]
        (testing "the cut-off is the fixture's median transcript day, so it prunes some but not all files"
          (is (= "claude daily --since 20260608" command))
          (is (< 0 pruned 5))))
      (testing "the recent cut-off is the fixture's 95th-percentile transcript day, so it prunes most files by mtime"
        (is (= "claude daily --since 20260613" (system/resolve-command (str "claude daily --since " data/recent-mtime) dir)))
        (is (some #{{:fixture :large :command (str "claude daily --since " data/recent-mtime)}} data/shape-commands)))
      (testing "an empty fixture cannot silently prune nothing"
        (is (thrown-with-msg? clojure.lang.ExceptionInfo #"No Claude transcripts"
                              (system/resolve-command (str "claude daily --since " data/median-mtime) (str (babashka.fs/path dir "none"))))))
      (testing "commands without the placeholder pass through"
        (is (= "claude daily" (system/resolve-command "claude daily" dir))))
      (finally (babashka.fs/delete-tree dir)))))

(deftest overall-verdicts
  (let [result (fn [cpu rss & [wall]] {:command "daily" :comparison {:wall {:verdict (or wall :unchanged)} :cpu {:verdict cpu} :rss {:verdict rss}}})]
    (is (:regression? (report/overall-verdict [(result :unchanged :regression)] limits)))
    (is (:regression? (report/overall-verdict [(result :improvement :unchanged :regression)] limits)))
    (is (= "**Verdict: no significant change** (every command within ±5% wall time and ±10% CPU time and peak RSS)."
           (:text (report/overall-verdict [(result :unchanged :unchanged)] limits))))
    (is (not (:regression? (report/overall-verdict [(result :improvement :unchanged)] limits))))
    (is (.contains (:text (report/overall-verdict [(result :unchanged :unchanged :regression)] limits))
                   "wall time above +5% or CPU time or peak RSS above +10%"))
    (testing "a memory-only improvement is reported"
      (is (= "**Verdict: ✅ improved** in `daily` (less memory); nothing regressed beyond the thresholds."
             (:text (report/overall-verdict [(result :unchanged :improvement)] limits)))))))

(deftest history-instruction-columns
  (let [with-instructions (fn [r n] (-> r (assoc-in ["base" :instructions] 100) (assoc-in ["PR" :instructions] n)))
        variants [{:label "base"} {:label "PR"}]
        rounds [(round 1000 1000) (round 1000 1000) (round 1000 1000)]]
    (is (not (.contains (history/render-history-table {:command "daily" :rounds rounds} variants limits) "Instructions")))
    (is (.contains (history/render-history-table {:command "daily" :rounds (mapv #(with-instructions % 50) rounds)} variants limits)
                   "| Instructions vs prev |"))))

(deftest github-warning-goes-to-stdout
  (let [err (java.io.StringWriter.)
        regression {:regression? true :text "**Verdict: ⚠️ possible regression** in `daily`"}
        out (binding [*err* err] (with-out-str (main/github-warning! regression true)))]
    (is (= "::warning title=ccusage performance::Verdict: ⚠️ possible regression in `daily`\n" out))
    (is (= "" (str err))))
  (is (= "" (with-out-str (main/github-warning! {:regression? true :text "x"} false))))
  (is (= "" (with-out-str (main/github-warning! {:text "x"} true)))))

(deftest file-lock-serializes-processes
  (let [dir (babashka.fs/create-temp-dir {:prefix "ccusage-lock-test."})
        log (system/path dir "events.log")
        script (str "(require '[compare-pr-performance.history :as history])"
                    " (let [id (first *command-line-args*)]"
                    "  (history/with-file-lock " (pr-str (system/path dir "build.lock"))
                    "   #(do (spit " (pr-str log) " (str \"in \" id \"\\n\") :append true) (Thread/sleep 300)"
                    "        (spit " (pr-str log) " (str \"out \" id \"\\n\") :append true))))")
        bb (str (babashka.fs/which "bb"))
        run (fn [id] (babashka.process/process [bb "--classpath" (str script-dir) "-e" script id]))]
    (try
      (run! deref (mapv run ["1" "2" "3"]))
      (let [events (map #(clojure.string/split % #" ") (clojure.string/split-lines (slurp log)))]
        (is (= 6 (count events)))
        (is (every? (fn [[[a x] [b y]]] (and (= "in" a) (= "out" b) (= x y))) (partition 2 events))))
      (finally (babashka.fs/delete-tree dir)))))

(deftest codex-fixture-compatibility
  (let [archived-pr "#1230" zstd-pr "#1890"
        support {"v20.0.5" #{} "v20.0.30" #{archived-pr} "main" #{archived-pr zstd-pr}}
        reads? (fn [variant input-class] (contains? (get support (:label variant) #{}) (:pr input-class)))
        v5 {:label "v20.0.5"} v30 {:label "v20.0.30"} main {:label "main"} binary {:label "/bin/ccusage"}
        with-fixture (fn [files f]
                       (let [dir (babashka.fs/create-temp-dir {:prefix "ccusage-codex-fixture."})]
                         (try
                           (doseq [file files]
                             (babashka.fs/create-dirs (babashka.fs/parent (system/path dir file)))
                             (spit (system/path dir file) ""))
                           (f (str dir))
                           (finally (babashka.fs/delete-tree dir)))))]
    (testing "compressed coverage stays when every ref reads every input"
      (is (= {:layout "current" :skipped []} (history/codex-compatibility reads? [main main] nil))))
    (testing "a ref that skips any input switches the generated fixture to the legacy layout"
      (let [result (history/codex-compatibility reads? [v30 main] nil)]
        (is (= "legacy" (:layout result)))
        (is (= [{:pr zstd-pr :refs [v30]}] (mapv #(select-keys % [:pr :refs]) (:skipped result))))
        (is (.contains (:note result) "`v20.0.30` cannot read zstd-compressed rollouts"))
        (is (not (.contains (:note result) "archived sessions"))))
      (is (.contains (:note (history/codex-compatibility reads? [v5 main] nil)) "`v20.0.5` cannot read archived sessions"))
      (is (= "legacy" (:layout (history/codex-compatibility reads? [binary main] nil)))))
    (testing "a custom fixture warns for each input class it contains that a ref skips"
      (with-fixture ["sessions/2026/06/01/rollout-a.jsonl"]
        #(is (nil? (:warning (history/codex-compatibility reads? [v5 main] %)))))
      (with-fixture ["archived_sessions/rollout-a.jsonl"]
        (fn [dir]
          (let [warning (:warning (history/codex-compatibility reads? [v5 main] dir))]
            (is (.contains warning "`v20.0.5` cannot read archived sessions"))
            (is (not (.contains warning "zstd"))))
          (is (nil? (:warning (history/codex-compatibility reads? [v30 main] dir))))))
      (with-fixture ["sessions/2026/06/01/rollout-a.jsonl.zst"]
        #(is (.contains (:warning (history/codex-compatibility reads? [v30 main] %)) "`v20.0.30` cannot read zstd-compressed rollouts")))
      (with-fixture ["archived_sessions/rollout-a.jsonl.zst"]
        #(is (= {:layout "current" :skipped []} (history/codex-compatibility reads? [main] %)))))
    (testing "only files the Codex loader reads as rollouts count"
      (with-fixture ["sessions/2026/06/01/cache.bin.zst" "sessions/notes.json.zst" "other/rollout-a.jsonl.zst"
                     "archived_sessions/README.md" "sessions/2026/06/01/rollout-a.jsonl"]
        #(is (nil? (:warning (history/codex-compatibility reads? [v5 main] %)))))
      (let [parent (babashka.fs/create-temp-dir {:prefix "ccusage-codex-fixture."})
            dir (system/path parent "archived_sessions" "codex")]
        (try
          (babashka.fs/create-dirs (system/path dir "sessions"))
          (spit (system/path dir "sessions" "rollout-a.jsonl") "")
          (is (nil? (:warning (history/codex-compatibility reads? [v5 main] dir))))
          (finally (babashka.fs/delete-tree parent)))))))

(deftest history-reuses-input-compatibility
  (let [dir (fs/create-temp-dir {:prefix "ccusage-history-compatibility."})
        cache (system/path dir "cache")
        claude (system/path dir "claude")
        codex (system/path dir "codex")
        checks (atom [])
        old-commit (apply str (repeat 40 "1"))
        new-commit (apply str (repeat 40 "2"))]
    (try
      (doseq [directory [claude codex]] (fs/create-dirs directory))
      (doseq [commit [old-commit new-commit]]
        (let [bin (system/path cache "bin" commit "ccusage")]
          (fs/create-dirs (fs/parent bin))
          (spit bin "cached binary")))
      (let [output
            (with-redefs [system/run-process
                          (fn [argv & _]
                            (case (first argv)
                              "git"
                              (case (second argv)
                                "rev-parse" {:exit 0 :out (case (last argv)
                                                          "--show-toplevel" (str dir)
                                                          "old^{commit}" old-commit
                                                          "new^{commit}" new-commit)
                                             :err ""}
                                "merge-base" (do (swap! checks conj (vec (take-last 2 argv)))
                                                 {:exit (if (= old-commit (last argv)) 1 0) :out "" :err ""}))
                              "bash"
                              (do (spit (nth argv 4) "0.100 0.050 0.010\n")
                                  (spit (nth argv 10) "1024\n")
                                  {:exit 0 :out "" :err ""})
                              "hyperfine"
                              (let [export (second (drop-while #(not= "--export-json" %) argv))]
                                (spit export (hyperfine-export [(hyperfine-result "old" 0.1) (hyperfine-result "new" 0.1)]))
                                {:exit 0 :out "" :err ""})))]
              (with-out-str
                (history/history-main "--real" "--runs" "3" "--warmup" "0" "--commands" "daily"
                                      "--fixture-dir" claude "--codex-fixture-dir" codex
                                      "--cache-dir" cache "old" "new")))]
        (is (= 4 (count @checks)))
        (is (every? #(= 1 %) (vals (frequencies @checks))))
        (is (.contains output
                       "⚠️ **Warning:** in local logs, `old` skip archived sessions (`archived_sessions/`); `old` skip zstd-compressed rollouts (`.jsonl.zst`), so Codex and unified rows across those changes compare different input."))
        (is (not (.contains output "the custom Codex fixture contains input some refs skip"))))
      (finally (fs/delete-tree dir)))))

(let [{:keys [fail error]} (apply run-tests ['compare-pr-performance-test])]
  (when (pos? (+ fail error))
    (System/exit 1)))
