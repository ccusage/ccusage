#!/usr/bin/env nix
#! nix shell --inputs-from ../../.. nixpkgs#babashka-unwrapped --command bb

(ns compare-pr-performance-test
  (:require [babashka.fs]
            [clojure.test :refer [deftest is run-tests testing]]))

(load-file (str (.getParent (babashka.fs/file *file*)) "/bench-history.bb"))

(def required-args ["--base-bin" "base" "--head-bin" "head" "--fixture-dir" "claude" "--codex-fixture-dir" "codex"])

(deftest cli-parsing
  (testing "defaults"
    (is (= {:runs 11 :warmup 2 :threshold 0.1}
           (select-keys (parse-cli required-args) [:runs :warmup :threshold]))))
  (testing "too few rounds to judge a regression"
    (is (thrown-with-msg? clojure.lang.ExceptionInfo #"--runs must be at least 3"
                          (parse-cli (conj required-args "--runs" "2")))))
  (testing "missing binary"
    (is (thrown-with-msg? clojure.lang.ExceptionInfo #"--head-bin is required"
                          (parse-cli ["--base-bin" "base" "--fixture-dir" "claude" "--codex-fixture-dir" "codex"]))))
  (testing "history takes refs as positional arguments"
    (is (= ["v20.0.20" "main"] (:refs (parse-history-cli ["--real" "v20.0.20" "main"]))))
    (is (true? (:real (parse-history-cli ["--real" "main"]))))
    (is (thrown-with-msg? clojure.lang.ExceptionInfo #"at least one git ref"
                          (parse-history-cli []))))
  (testing "history labels stay unique"
    (is (thrown-with-msg? clojure.lang.ExceptionInfo #"may appear once: main"
                          (parse-history-cli ["main" "v20.0.20" "main"])))
    (is (= ["/a/ccusage" "/b/ccusage"] (:refs (parse-history-cli ["/a/ccusage" "/b/ccusage"])))))
  (testing "a custom Claude fixture needs its Codex fixture"
    (is (thrown-with-msg? clojure.lang.ExceptionInfo #"--fixture-dir and --codex-fixture-dir together"
                          (parse-history-cli ["--fixture-dir" "c" "main"])))
    (is (thrown-with-msg? clojure.lang.ExceptionInfo #"--fixture-dir and --codex-fixture-dir together"
                          (parse-history-cli ["--codex-fixture-dir" "x" "main"]))))
  (testing "history rejects too few rounds instead of measuring them"
    (is (thrown-with-msg? clojure.lang.ExceptionInfo #"--runs must be at least 3"
                          (parse-history-cli ["--runs" "2" "main"])))
    (is (= 3 (:runs (parse-history-cli ["--runs" "3" "main"]))))))

(deftest formatting
  (is (= "999ms" (format-duration 999)))
  (is (= "1.00s" (format-duration 1000)))
  (is (= "1.50 KiB" (format-size 1536)))
  (is (= "0123456789ab" (format-sha "0123456789abcdef")))
  (is (= "1.00 (0.90–1.20)" (format-ratio-spread {:median 1.0 :min 0.9 :max 1.2}))))

(deftest command-rendering
  (is (= "env CLAUDE_CONFIG_DIR=/tmp/a \"CODEX_HOME=/tmp/codex data\" /tmp/ccusage claude daily --offline --json"
         (benchmark-command-text
          {:vars {"CLAUDE_CONFIG_DIR" "/tmp/a" "CODEX_HOME" "/tmp/codex data"}}
          (command-argv "/tmp/ccusage" "claude daily"))))
  (testing "fixture runs start from an empty environment"
    (is (= "env -i CLAUDE_CONFIG_DIR=/f/c CODEX_HOME=/f/x HOME=/e COLUMNS=200 LOG_LEVEL=0 NO_COLOR=1 TZ=UTC /b daily --offline --json"
           (benchmark-command-text (fixture-env "/f/c" "/f/x" "/e") (command-argv "/b" "daily"))))))

(deftest statistics
  (is (= 4 (median [9 1 2 4 7])))
  (is (= 3.0 (median [9 1 2 4])))
  (is (= {:median 3.0 :min 1 :max 9 :samples 4} (summarize [9 1 2 4])))
  (is (= ["b" "c" "a"] (rotate ["a" "b" "c"] 1)))
  (is (= ["a" "b" "c"] (rotate ["a" "b" "c"] 3))))

(deftest regression-classification
  (testing "a consistent slowdown past the threshold"
    (is (= :regression (classify-ratios [1.5 1.6 1.4 1.55 1.5 1.45 1.6 1.5 1.52 1.48 1.5] 0.1))))
  (testing "a consistent speedup"
    (is (= :improvement (classify-ratios [0.6 0.62 0.58 0.61 0.6] 0.1))))
  (testing "noise around 1.0 stays unchanged"
    (is (= :unchanged (classify-ratios [1.03 0.97 1.05 0.99 1.02 0.98 1.04 1.01 0.96 1.0 1.02] 0.1))))
  (testing "one outlier round cannot flag a regression"
    (is (= :unchanged (classify-ratios [1.0 1.02 0.98 1.01 2.5 0.99 1.0 1.03 0.97 1.0 1.01] 0.1))))
  (testing "a large median without agreement across rounds stays unchanged"
    (is (= :unchanged (classify-ratios [1.3 1.3 1.3 0.9 0.9 1.3 0.9] 0.1)))))

(defn round [base-cpu head-cpu]
  {"base" {:wall (* 0.2 base-cpu) :cpu base-cpu :rss 100 :instructions nil}
   "PR" {:wall (* 0.2 head-cpu) :cpu head-cpu :rss 100 :instructions nil}})

(deftest paired-comparison
  (let [rounds [(round 1000 1500) (round 1100 1600) (round 900 1400) (round 1000 1550) (round 1050 1500)]
        comparison (compare-variants rounds "base" "PR" 0.1)]
    (is (= :regression (get-in comparison [:cpu :verdict])))
    (is (= :unchanged (get-in comparison [:rss :verdict])))
    (is (nil? (:instructions comparison)))
    (is (= "⚠️ slower" (comparison-verdict comparison)))
    (is (= 1000 (get-in (variant-summary rounds "base") [:cpu :median])))))

(defn with-hyperfine-export [content f]
  (let [dir (babashka.fs/create-temp-dir {:prefix "ccusage-hyperfine-test."})
        output (path dir "hyperfine.json")]
    (try
      (spit output content)
      (f output)
      (finally
        (babashka.fs/delete-tree dir)))))

(defn hyperfine-result [name wall]
  {:name name :command "x"
   :measurements [{:time_wall_clock {:value wall :unit "second"} :time_cpu {:value (* 4 wall) :unit "second"}
                   :memory_peak_resident {:value 1048576.0 :unit "byte"} :exit_code 0}]})

(defn hyperfine-export [results]
  (cheshire.core/generate-string {:schema_version 2 :results results}))

(deftest hyperfine-runs
  (with-hyperfine-export
    (hyperfine-export [(hyperfine-result "PR" 0.2) (hyperfine-result "base" 0.1)])
    (fn [output]
      (is (= {"base" {:wall 100.0 :cpu 400.0 :rss 1048576.0 :instructions nil}
              "PR" {:wall 200.0 :cpu 800.0 :rss 1048576.0 :instructions nil}}
             (read-hyperfine-runs output ["base" "PR"])))))
  (with-hyperfine-export
    "not json"
    (fn [output]
      (is (thrown-with-msg? clojure.lang.ExceptionInfo #"Malformed hyperfine output"
                            (read-hyperfine-runs output ["base"])))))
  (with-hyperfine-export
    (cheshire.core/generate-string {:results []})
    (fn [output]
      (is (thrown-with-msg? clojure.lang.ExceptionInfo #"schema version"
                            (read-hyperfine-runs output ["base"])))))
  (with-hyperfine-export
    (hyperfine-export [(hyperfine-result "base" 0.1)])
    (fn [output]
      (is (thrown-with-msg? clojure.lang.ExceptionInfo #"0 results named PR"
                            (read-hyperfine-runs output ["base" "PR"])))))
  (with-hyperfine-export
    (hyperfine-export [(hyperfine-result "base" 0.1) (hyperfine-result "base" 0.2)])
    (fn [output]
      (is (thrown-with-msg? clojure.lang.ExceptionInfo #"2 results named base"
                            (read-hyperfine-runs output ["base"])))))
  (with-hyperfine-export
    (hyperfine-export [(assoc-in (hyperfine-result "base" 0.1) [:measurements 0 :time_cpu :unit] "millisecond")])
    (fn [output]
      (is (thrown-with-msg? clojure.lang.ExceptionInfo #"Unexpected hyperfine time unit"
                            (read-hyperfine-runs output ["base"]))))))

(deftest report-rendering
  (let [rounds [(round 1000 600) (round 1000 620) (round 1000 610)]
        result {:command "claude daily" :base (variant-summary rounds "base") :head (variant-summary rounds "PR")
                :comparison (compare-variants rounds "base" "PR" 0.1)}
        markdown (render-report {:base-sha "1111111111111111" :head-sha "2222222222222222" :threshold 0.1 :runs 3 :warmup 1
                                 :fixture-stats {:bytes 1073741824 :files 10} :codex-fixture-stats {:bytes 1048576 :files 2}}
                                [result] {:base 2048 :head 1024})]
    (is (.startsWith markdown "<!-- ccusage-perf-comment -->\n<!-- ccusage-perf-comment:2222222222222222 -->\n"))
    (is (.contains markdown "PR: `222222222222` · Base: `111111111111`"))
    (is (.contains markdown "**Verdict: ✅ improved** in `claude daily` (faster)"))
    (is (.contains markdown "| `claude daily` | 1.00s | 610ms | 0.61 (0.60–0.62) |"))
    (is (.contains markdown "| release binary | 2.00 KiB | 1.00 KiB | -1.00 KiB |"))))

(deftest overall-verdicts
  (let [result (fn [cpu rss] {:command "daily" :comparison {:cpu {:verdict cpu} :rss {:verdict rss}}})]
    (is (:regression? (overall-verdict [(result :unchanged :regression)] 0.1)))
    (is (.contains (:text (overall-verdict [(result :unchanged :unchanged)] 0.1)) "no significant change"))
    (is (not (:regression? (overall-verdict [(result :improvement :unchanged)] 0.1))))
    (testing "a memory-only improvement is reported"
      (is (= "**Verdict: ✅ improved** in `daily` (less memory); nothing regressed beyond 10%."
             (:text (overall-verdict [(result :unchanged :improvement)] 0.1)))))))

(deftest history-instruction-columns
  (let [with-instructions (fn [r n] (-> r (assoc-in ["base" :instructions] 100) (assoc-in ["PR" :instructions] n)))
        variants [{:label "base"} {:label "PR"}]
        rounds [(round 1000 1000) (round 1000 1000) (round 1000 1000)]]
    (is (not (.contains (render-history-table {:command "daily" :rounds rounds} variants 0.1) "Instructions")))
    (is (.contains (render-history-table {:command "daily" :rounds (mapv #(with-instructions % 50) rounds)} variants 0.1)
                   "| Instructions vs prev |"))))

(deftest github-warning-goes-to-stdout
  (let [err (java.io.StringWriter.)
        regression {:regression? true :text "**Verdict: ⚠️ possible regression** in `daily`"}
        out (binding [*err* err] (with-out-str (github-warning! regression true)))]
    (is (= "::warning title=ccusage performance::Verdict: ⚠️ possible regression in `daily`\n" out))
    (is (= "" (str err))))
  (is (= "" (with-out-str (github-warning! {:regression? true :text "x"} false))))
  (is (= "" (with-out-str (github-warning! {:text "x"} true)))))

(let [{:keys [fail error]} (apply run-tests ['compare-pr-performance-test])]
  (when (pos? (+ fail error))
    (System/exit 1)))
