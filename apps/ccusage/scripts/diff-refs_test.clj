#!/usr/bin/env nix
#! nix shell --inputs-from ../../.. nixpkgs#babashka-unwrapped nixpkgs#git nixpkgs#jq nixpkgs#diffutils --command bb

(ns diff-refs-test
  (:require [babashka.fs]
            [clojure.string]
            [clojure.test :refer [deftest is run-tests testing]]))

(load-file (str (.getParent (babashka.fs/file *file*)) "/diff-refs.bb"))

(deftest cli-parsing
  (testing "two positional refs or binaries, the same one allowed twice"
    (is (= ["HEAD" "HEAD"] (:refs (parse-diff-cli ["HEAD" "HEAD" "--real"]))))
    (is (true? (:real (parse-diff-cli ["--real" "main" "HEAD"]))))
    (is (true? (:allow-stderr-diff (parse-diff-cli ["--allow-stderr-diff" "main" "HEAD"])))))
  (testing "window bounds stay strings"
    (is (= {:since "20250101" :until "20250131"}
           (select-keys (parse-diff-cli ["--since" "20250101" "--until" "20250131" "a" "b"]) [:since :until]))))
  (testing "rejects anything but a base and a head"
    (is (thrown-with-msg? clojure.lang.ExceptionInfo #"exactly two" (parse-diff-cli ["main"])))
    (is (thrown-with-msg? clojure.lang.ExceptionInfo #"exactly two" (parse-diff-cli ["a" "b" "c"]))))
  (testing "real logs and a fixture cannot be combined"
    (is (thrown-with-msg? clojure.lang.ExceptionInfo #"mutually exclusive"
                          (parse-diff-cli ["--real" "--fixture" "/tmp" "a" "b"])))))

(deftest report-matrix
  (is (= ["daily" "weekly" "monthly" "session"] (take 4 report-commands)))
  (is (some #{"claude blocks"} report-commands))
  (is (some #{"opencode weekly"} report-commands))
  (is (some #{"cline weekly"} report-commands))
  (testing "agents without weekly, blocks, or statusline"
    (is (= ["codex daily" "codex monthly" "codex session"]
           (filter #(clojure.string/starts-with? % "codex ") report-commands)))
    (is (not-any? #(re-find #"statusline|^(codex|amp) (weekly|blocks)" %) report-commands)))
  (is (= 65 (count report-commands))))

(defn- args-of [plan id] (:args (first (filter #(= id (:id %)) plan))))

(deftest planned-commands
  (let [fixture (command-plan {} {:since "20250101" :until "20250131"})
        real (command-plan {:real true} (real-dates (java.time.LocalDate/of 2026 3 1)))]
    (is (= (* (count output-variants) (count report-commands)) (count fixture) (count real)))
    (is (apply distinct? (map :id fixture)))
    (testing "fixture runs are unbounded except for the window variant"
      (is (= ["claude" "daily" "--offline" "--timezone" "UTC" "--json"] (args-of fixture "claude-daily--json")))
      (is (= ["daily" "--offline" "--timezone" "UTC"] (args-of fixture "daily--table")))
      (is (= ["codex" "session" "--offline" "--timezone" "UTC" "--json" "--breakdown"]
             (args-of fixture "codex-session--json-breakdown")))
      (is (= ["claude" "blocks" "--offline" "--timezone" "UTC" "--json" "--since" "20250101" "--until" "20250131"]
             (args-of fixture "claude-blocks--json-window"))))
    (testing "every real run stops at yesterday"
      (is (every? #(= ["--until" "20260228"] (take-last 2 (:args %))) real))
      (is (= ["daily" "--offline" "--timezone" "UTC" "--json" "--since" "20260130" "--until" "20260228"]
             (args-of real "daily--json-window"))))
    (is (true? (:json? (first fixture))))
    (is (false? (:json? (second fixture))))))

(deftest cost-mode-and-timezone-commands
  (let [fixture (command-plan {} {:since "20250101" :until "20250131"})
        real (command-plan {:real true} (real-dates (java.time.LocalDate/of 2026 3 1)))]
    (doseq [command report-commands
            [variant timezone extra] [["json-mode-calculate" "UTC" ["--mode" "calculate"]]
                                      ["json-mode-display" "UTC" ["--mode" "display"]]
                                      ["json-tz-tokyo" "Asia/Tokyo" []]]]
      (let [id (str (clojure.string/replace command " " "-") "--" variant)
            args (vec (concat (clojure.string/split command #" ")
                              ["--offline" "--timezone" timezone "--json"] extra))]
        (is (= args (args-of fixture id)) id)
        (is (= (into args ["--until" "20260228"]) (args-of real id)) id)))))

(deftest environments
  (let [fixture (babashka.fs/create-temp-dir {:prefix "ccusage-diff-fixture."})]
    (try
      (babashka.fs/create-dirs (path fixture "claude"))
      (babashka.fs/create-dirs (path fixture "amp"))
      (spit (path fixture "codex") "a file is not an agent directory")
      (let [env (hermetic-env (str fixture) "/e")]
        (is (true? (:clean? env)))
        (is (= {"HOME" "/e" "XDG_CACHE_HOME" "/e/.cache" "XDG_CONFIG_HOME" "/e/.config" "XDG_DATA_HOME" "/e/.local/share"
                "CLAUDE_CONFIG_DIR" (path fixture "claude") "AMP_DATA_DIR" (path fixture "amp")
                "COLUMNS" "200" "NO_COLOR" "1" "TZ" "UTC"}
               (:vars env)))
        (testing "nothing from the caller leaks into a hermetic run"
          (is (= (:vars env) (process-env env {"CODEX_HOME" "/real" "PATH" "/bin"})))))
      (finally (babashka.fs/delete-tree fixture))))
  (testing "a real run inherits the caller's environment at the default log level"
    (is (= {"HOME" "/h" "CODEX_HOME" "/real" "COLUMNS" "200" "NO_COLOR" "1" "TZ" "UTC"}
           (process-env (real-env) {"HOME" "/h" "CODEX_HOME" "/real" "LOG_LEVEL" "0" "COLUMNS" "80"})))))

(defn- run [exit out err] {:exit exit :out (.getBytes out) :err (.getBytes err)})

(deftest classification
  (let [base (run 0 "a" "w")
        verdict (fn [runs allow-stderr-diff] (select-keys (classify runs allow-stderr-diff) [:status :streams]))]
    (is (= {:status :identical :streams []} (verdict {:base [base (run 0 "a" "w")] :head [(run 0 "a" "w")]} false)))
    (is (= {:status :different :streams [:stdout]} (verdict {:base [base base] :head [(run 0 "b" "w")]} false)))
    (is (= {:status :different :streams [:exit :stderr]} (verdict {:base [base base] :head [(run 1 "a" "x")]} false)))
    (testing "stderr is ignored on request, the exit code never"
      (is (= :identical (:status (classify {:base [base base] :head [(run 0 "a" "x")]} true))))
      (is (= {:status :different :streams [:exit]} (verdict {:base [base base] :head [(run 1 "a" "x")]} true))))
    (testing "a base that disagrees with itself is unstable, whatever head printed"
      (is (= {:status :unstable :streams [:stdout]} (verdict {:base [base (run 0 "c" "w")] :head [(run 0 "b" "w")]} false)))
      (is (= {:status :unstable :streams [:stdout]}
             (verdict {:base [base base base (run 0 "c" "w")] :head [(run 0 "b" "w") base base]} false)))
      (is (= :identical (:status (classify {:base [base (run 0 "a" "y")] :head [base]} true)))))
    (testing "a head that only sometimes disagrees with a reproducible base is still different"
      (let [odd (run 0 "b" "w")
            result (classify {:base [base base base base] :head [base base odd]} false)]
        (is (= :different (:status result)))
        (is (= [base odd] (:pair result)))))))

(deftest summary
  (is (= {"Commands run" 4 "Identical" 2 "Different" 1 "Unstable" 1}
         (summary-counts [{:status :identical} {:status :different} {:status :identical} {:status :unstable}]))))

(deftest end-to-end
  (let [dir (babashka.fs/create-temp-dir {:prefix "ccusage-diff-e2e."})
        script (fn [name body]
                 (let [file (path dir name)]
                   (spit file (str "#!/bin/sh\nPATH=/no-such-path\n" body "\n"))
                   (babashka.fs/set-posix-file-permissions file "rwx------")
                   file))
        stable (script "stable" "echo '{\"a\":1,\"b\":2}'")
        reordered (script "reordered" "echo '{\"b\":2,\"a\":1}'")
        changed (script "changed" "echo '{\"b\":2,\"a\":3}'; echo warn >&2")
        intermittent (script "intermittent"
                             (str "if [ -f " (path dir "head-ran") " ]; then echo '{\"a\":3,\"b\":2}'; "
                                  "else : > " (path dir "head-ran") "; echo '{\"a\":1,\"b\":2}'; fi"))
        flaky (script "flaky" (str "n=0; if [ -f " (path dir "calls") " ]; then read -r n < " (path dir "calls")
                                   "; fi; n=$((n + 1)); echo \"$n\" > " (path dir "calls") "; echo \"$n\""))
        compare (fn [bins allow-stderr-diff]
                  (compare-command! bins (hermetic-env nil (str dir)) (str dir) (str dir) allow-stderr-diff
                                    {:id "daily--json" :json? true :args ["daily"]}))]
    (try
      (testing "identical output leaves nothing behind"
        (let [result (compare {:base stable :head stable} false)]
          (is (= :identical (:status result)))
          (is (not (babashka.fs/exists? (:dir result))))))
      (testing "head must agree on repeated runs even when its first run matches"
        (let [result (compare {:base stable :head intermittent} false)]
          (is (= :different (:status result)))
          (is (= [:stdout] (:streams result)))))
      (testing "a difference keeps both outputs and diffs them key-sorted"
        (let [result (compare {:base stable :head changed} false)
              [base head] (:pair result)
              diff (unified-diff (:out-file base) (:out-file head) true)]
          (is (= :different (:status result)))
          (is (= [:stdout :stderr] (:streams result)))
          (is (= ["base run 1" "head run 1"] [(:label base) (:label head)]))
          (is (babashka.fs/exists? (:err-file head)))
          (is (.contains diff "-  \"a\": 1"))
          (is (.contains diff "+  \"a\": 3"))
          (is (not-any? #(clojure.string/starts-with? % "+  \"b\":")
                        (clojure.string/split-lines diff)))))
      (testing "raw-byte differences remain visible when sorted JSON matches"
        (let [result (compare {:base stable :head reordered} false)
              [base head] (:pair result)
              diff (unified-diff (:out-file base) (:out-file head) true)]
          (is (= :different (:status result)))
          (is (.contains diff "-{\"a\":1,\"b\":2}"))
          (is (.contains diff "+{\"b\":2,\"a\":1}"))))
      (testing "the stderr difference alone is tolerated on request"
        (is (= [:stdout] (:streams (compare {:base stable :head changed} true)))))
      (testing "a binary that never prints the same thing twice is unstable"
        (is (= :unstable (:status (compare {:base flaky :head flaky} false)))))
      (finally (babashka.fs/delete-tree dir)))))

(deftest inconclusive-comparisons-exit-nonzero
  (let [dir (babashka.fs/create-temp-dir {:prefix "ccusage-diff-status."})
        status (atom nil)]
    (try
      (with-redefs [resolve-variants (fn [& _] [{:bin "base"} {:bin "head"}])
                    run-all! (fn [& _] [{:status :unstable :args ["daily"]}])
                    write-progress (fn [_])]
        (with-out-str
          (reset! status (diff-main "HEAD" "HEAD" "--output-dir" (str dir)))))
      (is (= 2 @status))
      (finally (babashka.fs/delete-tree dir)))))

(let [{:keys [fail error]} (apply run-tests ['diff-refs-test])]
  (when (pos? (+ fail error))
    (System/exit 1)))
