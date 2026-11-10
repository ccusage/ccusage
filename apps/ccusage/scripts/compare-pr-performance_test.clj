#!/usr/bin/env nix
#! nix shell --inputs-from ../../.. nixpkgs#babashka-unwrapped --command bb

(ns compare-pr-performance-test
  (:require [babashka.fs]
            [clojure.test :refer [deftest is run-tests testing]]))

(load-file (str (.getParent (babashka.fs/file *file*)) "/compare-pr-performance.bb"))

(deftest cli-parsing
  (testing "defaults and required options"
    (is (= {:head-runtime "package"
            :runs 7
            :warmup 2
            :large-runs 1
            :large-warmup 0
            :memory-runs 1
            :package-runner-timeout-ms 120000}
           (select-keys
            (parse-cli ["--base-dir" "." "--head-dir" "."
                        "--fixture-dir" "claude" "--codex-fixture-dir" "codex"])
            [:head-runtime :runs :warmup :large-runs :large-warmup
             :memory-runs :package-runner-timeout-ms]))))
  (testing "invalid sample counts"
    (is (thrown-with-msg? clojure.lang.ExceptionInfo
                          #"--runs must be a positive integer"
                          (parse-cli ["--base-dir" "." "--head-dir" "."
                                      "--fixture-dir" "claude" "--codex-fixture-dir" "codex"
                                      "--runs" "0"]))))
  (testing "missing required fixture option"
    (is (thrown-with-msg? clojure.lang.ExceptionInfo
                          #"--fixture-dir is required"
                          (parse-cli ["--base-dir" "." "--head-dir" "."
                                      "--codex-fixture-dir" "codex"])))))

(deftest runtime-choice-parsing
  (is (= "package" (parse-head-runtime "package")))
  (is (= "rust" (parse-head-runtime "rust")))
  (is (thrown-with-msg? clojure.lang.ExceptionInfo
                        #"Invalid head runtime: js. Use package or rust."
                        (parse-head-runtime "js"))))

(deftest formatting
  (is (= "999.0ms" (format-duration 999)))
  (is (= "1.000s" (format-duration 1000)))
  (is (= "1.50 KiB" (format-size 1536)))
  (is (= "0123456789ab" (format-sha "0123456789abcdef")))
  (is (= "abcdef012345"
         (package-url-sha "https://pkg.pr.new/example/ccusage@abcdef0123456789")))
  (is (= "https://example.test/pkg.tgz"
         (package-url-sha "https://example.test/pkg.tgz"))))

(deftest command-rendering
  (is (= "env CLAUDE_CONFIG_DIR=/tmp/a \"CODEX_HOME=/tmp/codex data\" node /tmp/ccusage claude daily --offline --json"
         (benchmark-command-text
          {"CLAUDE_CONFIG_DIR" "/tmp/a" "CODEX_HOME" "/tmp/codex data"}
          ["node" "/tmp/ccusage" "claude" "daily" "--offline" "--json"]))))

(defn hyperfine-result [name median]
  {:command (str "sleep " median) :name name
   :summary {:time_wall_clock {:unit "second" :count 3 :median median
                               :min (- median 0.02) :max (+ median 0.02)}}})

(defn with-hyperfine-export [content f]
  (let [dir (babashka.fs/create-temp-dir {:prefix "ccusage-hyperfine-test."})
        output (path dir "hyperfine.json")]
    (try
      (spit output content)
      (f output)
      (finally
        (babashka.fs/delete-tree dir)))))

(defn hyperfine-export [results]
  (cheshire.core/generate-string {:schema_version 2 :primary_metric "time_wall_clock" :results results}))

(deftest hyperfine-normalization
  (is (= {:max 130.0 :median 110.0 :min 90.0 :samples 3}
         (measurement-from-hyperfine (hyperfine-result "base" 0.11))))
  (is (thrown-with-msg? clojure.lang.ExceptionInfo
                        #"unit"
                        (measurement-from-hyperfine
                         (assoc-in (hyperfine-result "base" 0.11) [:summary :time_wall_clock :unit] "millisecond")))))

(deftest hyperfine-results-follow-requested-names
  (with-hyperfine-export
    (hyperfine-export [(hyperfine-result "PR" 0.2) (hyperfine-result "base" 0.1)])
    (fn [output]
      (is (= ["base" "PR"] (map :name (read-hyperfine-results output ["base" "PR"])))))))

(deftest malformed-hyperfine-output
  (with-hyperfine-export
    "not json"
    (fn [output]
      (is (thrown-with-msg? clojure.lang.ExceptionInfo
                            #"Malformed hyperfine output"
                            (read-hyperfine-results output ["base" "PR"])))))
  (with-hyperfine-export
    (cheshire.core/generate-string {:results [{:mean 0.1 :times [0.1]}]})
    (fn [output]
      (is (thrown-with-msg? clojure.lang.ExceptionInfo
                            #"schema version"
                            (read-hyperfine-results output ["base"])))))
  (with-hyperfine-export
    (hyperfine-export [(hyperfine-result "base" 0.1) (hyperfine-result "base" 0.2)])
    (fn [output]
      (is (thrown-with-msg? clojure.lang.ExceptionInfo
                            #"2 results named base"
                            (read-hyperfine-results output ["base" "PR"])))))
  (with-hyperfine-export
    (hyperfine-export [(hyperfine-result "base" 0.1)])
    (fn [output]
      (is (thrown-with-msg? clojure.lang.ExceptionInfo
                            #"0 results named PR"
                            (read-hyperfine-results output ["base" "PR"]))))))

(deftest measurement-summary
  (is (= {:max 9 :median 4 :min 1 :samples 4}
         (measurement-from-milliseconds [9 1 2 4]))))

(deftest fallback-decisions
  (is (= :installed (base-source-decision {:package-install {:bin-entry "bin"}})))
  (is (= :local (base-source-decision {:base-dir "/repo"})))
  (is (= :skip (base-source-decision {:base-package-url "https://example.test/pkg"})))
  (is (= :remote (base-package-size-source {:base-package-install {:bin-entry "bin"}})))
  (is (= :local (base-package-size-source {:base-dir "/repo"
                                           :base-package-url "https://example.test/pkg"})))
  (is (= :remote (head-package-size-source {:head-package-install {:bin-entry "bin"}})))
  (is (= :local (head-package-size-source {:head-dir "/repo"
                                           :head-package-url "https://example.test/pkg"}))))

(deftest platform-normalization
  (is (= "darwin" (normalize-platform-name "Mac OS X")))
  (is (= "linux" (normalize-platform-name "Linux")))
  (is (= "win32" (normalize-platform-name "Windows 11")))
  (is (= "freebsd" (normalize-platform-name "FreeBSD"))))

(deftest packed-tarball-path-resolution
  (is (= "/tmp/pack/ccusage.tgz"
         (packed-tarball-path "/tmp/pack" "ccusage.tgz")))
  (is (= "/var/tmp/ccusage.tgz"
         (packed-tarball-path "/tmp/pack" "/var/tmp/ccusage.tgz"))))

(deftest skipped-markdown-rendering
  (let [markdown (render-skipped-markdown
                  {:base-package-url "https://pkg.pr.new/x/ccusage@abcdef0123456789"
                   :base-sha "1111111111111111"
                   :head-runtime "package"
                   :head-sha "2222222222222222"
                   :reason "Package unavailable."})]
    (is (.startsWith markdown "<!-- ccusage-perf-comment -->\n"))
    (is (.contains markdown "PR SHA: `222222222222`"))
    (is (.contains markdown "Base package: `abcdef012345`"))
    (is (.endsWith markdown "\n"))))

(deftest memory-columns-preserve-report-order
  (let [markdown (render-fixture-section
                  {:title "Fixture" :description "Description" :fixture-dir "/repo/fixture"
                   :fixture-stats {:bytes 1048576 :files 1} :memory-runs 1 :runs 1 :warmup 0
                   :results [{:command "claude daily"
                              :base {:median 20} :head {:median 10}
                              :base-memory {:peak-rss-bytes 2048}
                              :head-memory {:peak-rss-bytes 1024}}]}
                  {:head-dir "/repo" :head-runtime "rust"})]
    (is (.contains markdown
                   "| Command | Input | Base median | PR median | PR vs base | Base peak RSS | PR peak RSS | PR&#x2f;base RSS | Base throughput | PR throughput |"))))

(let [{:keys [fail error]} (apply run-tests ['compare-pr-performance-test])]
  (when (pos? (+ fail error))
    (System/exit 1)))
