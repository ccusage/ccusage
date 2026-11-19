#!/usr/bin/env nix
#! nix shell --inputs-from ../../.. nixpkgs#babashka-unwrapped nixpkgs#git nixpkgs#jq nixpkgs#diffutils --command bb

(ns diff-refs-test
  (:require [babashka.classpath :as cp]
            [babashka.fs :as fs]
            [cheshire.core]
            [clojure.string]
            [clojure.test :refer [deftest is run-tests testing]]))

(def script-dir (fs/parent (fs/absolutize *file*)))
(cp/add-classpath (str script-dir))

(require '[diff-refs.main :as diff]
         '[compare-pr-performance.history :as history]
         '[compare-pr-performance.system :as system])

(deftest cli-parsing
  (testing "two positional refs or binaries, the same one allowed twice"
    (is (= ["HEAD" "HEAD"] (:refs (diff/parse-diff-cli ["HEAD" "HEAD" "--real"]))))
    (is (true? (:real (diff/parse-diff-cli ["--real" "main" "HEAD"]))))
    (is (true? (:allow-stderr-diff (diff/parse-diff-cli ["--allow-stderr-diff" "main" "HEAD"])))))
  (testing "window bounds stay strings"
    (is (= {:since "20250101" :until "20250131"}
           (select-keys (diff/parse-diff-cli ["--since" "20250101" "--until" "20250131" "a" "b"]) [:since :until]))))
  (testing "rejects anything but a base and a head"
    (is (thrown-with-msg? clojure.lang.ExceptionInfo #"exactly two" (diff/parse-diff-cli ["main"])))
    (is (thrown-with-msg? clojure.lang.ExceptionInfo #"exactly two" (diff/parse-diff-cli ["a" "b" "c"]))))
  (testing "real logs and a fixture cannot be combined"
    (is (thrown-with-msg? clojure.lang.ExceptionInfo #"mutually exclusive"
                          (diff/parse-diff-cli ["--real" "--fixture" "/tmp" "a" "b"])))))

(defn- stub-binary
  [dir name body]
  (let [file (system/path dir name)]
    (spit file (str "#!/bin/sh\nPATH=/no-such-path\n" body "\n"))
    (babashka.fs/set-posix-file-permissions file "rwx------")
    file))

(def stub-help
  (str "case \"$*\" in\n"
       "  --help) printf 'USAGE:\\n  ccusage <COMMANDS>\\n\\nCOMMANDS:\\n  daily       Show\\n  blocks      Show\\n"
       "  statusline  Show\\n  claude      Show\\n  codex       Show\\n\\nOPTIONS:\\n  -j, --json  Output\\n'; exit 0;;\n"
       "  'claude --help') printf 'COMMANDS:\\n  daily       Show\\n  blocks      Show\\n  statusline  Show\\n'; exit 0;;\n"
       "  'codex --help') printf 'COMMANDS:\\n  daily    Show\\n  monthly  Show\\n'; exit 0;;\n"
       "  *--help) printf 'OPTIONS:\\n  -j, --json  Output\\n'; exit 0;;\n"
       "esac"))

(deftest report-matrix
  (let [dir (babashka.fs/create-temp-dir {:prefix "ccusage-diff-help."})]
    (try
      (testing "every report the head binary's help lists, statusline left out"
        (is (= ["daily" "blocks" "claude daily" "claude blocks" "codex daily" "codex monthly"]
               (diff/report-commands (stub-binary dir "cli" stub-help)))))
      (testing "a help page without commands cannot yield an empty plan"
        (is (thrown-with-msg? clojure.lang.ExceptionInfo #"lists no report commands"
                              (diff/report-commands (stub-binary dir "bare" "echo 'OPTIONS:'")))))
      (finally (babashka.fs/delete-tree dir)))))

(def planned ["daily" "weekly" "blocks" "claude daily" "claude blocks" "codex session"])

(defn- args-of [plan id] (:args (first (filter #(= id (:id %)) plan))))

(deftest planned-commands
  (let [fixture (diff/command-plan {} {:since "20250101" :until "20250131"} planned)
        real (diff/command-plan {:real true} (diff/real-dates (java.time.LocalDate/of 2026 3 1)) planned)]
    (is (= (* (count diff/output-variants) (count planned)) (count fixture) (count real)))
    (is (apply distinct? (map :id fixture)))
    (testing "fixture runs are unbounded except for the window variant"
      (is (= ["claude" "daily" "--offline" "--timezone" "UTC" "--json"] (args-of fixture "claude-daily--json")))
      (is (= ["daily" "--offline" "--timezone" "UTC"] (args-of fixture "daily--table")))
      (is (= ["blocks" "--offline" "--timezone" "UTC" "--json"] (args-of fixture "blocks--json")))
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
  (let [fixture (diff/command-plan {} {:since "20250101" :until "20250131"} planned)
        real (diff/command-plan {:real true} (diff/real-dates (java.time.LocalDate/of 2026 3 1)) planned)]
    (doseq [command planned
            [variant timezone extra] [["json-mode-calculate" "UTC" ["--mode" "calculate"]]
                                      ["json-mode-display" "UTC" ["--mode" "display"]]
                                      ["json-tz-tokyo" "Asia/Tokyo" []]]]
      (let [id (str (clojure.string/replace command " " "-") "--" variant)
            args (vec (concat (clojure.string/split command #" ")
                              ["--offline" "--timezone" timezone "--json"] extra))]
        (is (= args (args-of fixture id)) id)
        (is (= (into args ["--until" "20260228"]) (args-of real id)) id)))))

(def ^:private pinned-manifest
  {"cwd" "/f/cwd"
   "env" {"HOME" "/f/home" "CLAUDE_CONFIG_DIR" "/f/claude,/f/opencode/claude"
          "LOG_LEVEL" "0" "COLUMNS" "200" "CCUSAGE_TEST_NOW" "2026-01-02T12:00:00Z"}})

(deftest environments
  (let [env (diff/golden-env pinned-manifest true)]
    (is (true? (:clean? env)))
    (testing "the golden environment at the default log level"
      (is (= {"HOME" "/f/home" "CLAUDE_CONFIG_DIR" "/f/claude,/f/opencode/claude"
              "CCUSAGE_TEST_NOW" "2026-01-02T12:00:00Z" "COLUMNS" "200" "NO_COLOR" "1" "TZ" "UTC"}
             (:vars env))))
    (testing "nothing from the caller leaks into a fixture run"
      (is (= (:vars env) (diff/process-env env {"CODEX_HOME" "/real" "PATH" "/bin"})))))
  (testing "binaries that cannot pin the clock both run under the wall clock"
    (is (not (contains? (:vars (diff/golden-env pinned-manifest false)) "CCUSAGE_TEST_NOW"))))
  (testing "a real run inherits the caller's environment at the default log level and the wall clock"
    (is (= {"HOME" "/h" "CODEX_HOME" "/real" "COLUMNS" "200" "NO_COLOR" "1" "TZ" "UTC"}
           (diff/process-env (diff/real-env) {"HOME" "/h" "CODEX_HOME" "/real" "LOG_LEVEL" "0" "COLUMNS" "80"
                                    "CCUSAGE_TEST_NOW" "2026-01-02T12:00:00Z"})))))

(deftest pinned-clock-builds
  (testing "a ref is built with the test clock only when its ccusage-core declares it"
    (is (true? (diff/declares-test-clock? "[features]\ntest-snapshots = [\"dep:x\"]\ntest-clock = []\n")))
    (is (false? (diff/declares-test-clock? "[features]\ntest-snapshots = [\"dep:x\"]\n")))
    (is (false? (diff/declares-test-clock? nil))))
  (testing "refs use the binary feature when present, with the core feature for older refs"
    (doseq [[ref expected] [["old" ["ccusage-core/test-clock"]] ["new" ["ccusage/test-clock"]] ["none" []]]]
      (with-redefs [system/run-process
                    (fn [[_ _ source] _]
                      (if (#{"old:rust/crates/ccusage-core/Cargo.toml" "new:rust/crates/ccusage/Cargo.toml"} source)
                        {:exit 0 :out "[features]\ntest-clock = []\n"}
                        {:exit 1}))]
        (is (= expected (diff/clock-features "." ref))))))
  (testing "a test-clock build is cached apart from the plain release build"
    (is (= "abc" (history/build-key "abc" [])))
    (is (not= (history/build-key "abc" []) (history/build-key "abc" ["ccusage/test-clock"])))))

(defn- run-diff-main
  [variants]
  (let [dir (babashka.fs/create-temp-dir {:prefix "ccusage-diff-clock."})
        envs (atom []) progress (atom []) features-for (atom nil)]
    (try
      (with-redefs [history/resolve-variants (fn [_ _ _ f] (reset! features-for f) variants)
                    diff/prepare-golden-fixture! (fn [& _] (assoc pinned-manifest "cwd" (str dir)))
                    diff/report-commands (fn [_] ["daily"])
                    diff/compare-command! (fn [_ env & _] (swap! envs conj env) {:status :identical :args ["daily"]})
                    system/write-progress #(swap! progress conj %)]
        (with-out-str (diff/diff-main "HEAD" "HEAD" "--output-dir" (str dir))))
      {:envs @envs :progress @progress :features-for @features-for}
      (finally (babashka.fs/delete-tree dir)))))

(deftest pinned-clock-comparisons
  (testing "refs are built with the test clock where they declare it"
    (let [features-for (:features-for (run-diff-main [{:bin "b" :features []} {:bin "h" :features []}]))]
      (is (some #{(first (features-for "HEAD"))} diff/test-clock-features))))
  (testing "both sides pinned keep the golden clock"
    (let [{:keys [envs]} (run-diff-main [{:bin "b" :features ["ccusage-core/test-clock"]} {:bin "h" :features ["ccusage/test-clock"]}])]
      (is (seq envs))
      (is (every? #(= "2026-01-02T12:00:00Z" (get-in % [:vars "CCUSAGE_TEST_NOW"])) envs))))
  (testing "one side that cannot pin it moves both to the wall clock, with a notice naming it"
    (let [{:keys [envs progress]} (run-diff-main [{:label "v18.0.0" :bin "b" :features []}
                                                  {:label "HEAD" :bin "h" :features ["ccusage/test-clock"]}])]
      (is (seq envs))
      (is (not-any? #(contains? (:vars %) "CCUSAGE_TEST_NOW") envs))
      (is (some #(and (.contains ^String % "v18.0.0") (.contains ^String % "wall clock")) progress)))))

(defn- golden-manifest
  [dir]
  (let [store (doto (system/path dir "claude") babashka.fs/create-dirs)]
    {"cwd" (str dir)
     "env" {"HOME" (str dir) "CLAUDE_CONFIG_DIR" (str " " store " ," (system/path dir "missing"))}
     "seeded" ["CLAUDE_CONFIG_DIR"]
     "window" {"since" "20251231" "until" "20260101"}}))

(defn- write-manifest! [dir manifest]
  (babashka.fs/create-dirs dir)
  (spit (system/path dir diff/fixture-manifest) (if (string? manifest) manifest (cheshire.core/generate-string manifest))))

(deftest golden-fixtures
  (let [dir (babashka.fs/create-temp-dir {:prefix "ccusage-diff-fixture."})
        exports (atom [])]
    (try
      (with-redefs [diff/export-golden-fixture! (fn [_ target]
                                             (swap! exports conj target)
                                             (write-manifest! target (golden-manifest target)))]
        (testing "a missing or empty directory is exported into, an exported one reused"
          (let [exported (diff/prepare-golden-fixture! "/repo" (system/path dir "new"))]
            (is (= (golden-manifest (system/path dir "new")) exported (diff/prepare-golden-fixture! "/repo" (system/path dir "new")))))
          (babashka.fs/create-dirs (system/path dir "empty"))
          (diff/prepare-golden-fixture! "/repo" (system/path dir "empty"))
          (is (= [(system/path dir "new") (system/path dir "empty")] @exports)))
        (testing "a directory holding something else is never written into"
          (babashka.fs/create-dirs (system/path dir "unrelated"))
          (spit (system/path dir "unrelated" "notes") "keep")
          (is (thrown-with-msg? clojure.lang.ExceptionInfo #"is not a golden fixture"
                                (diff/prepare-golden-fixture! "/repo" (system/path dir "unrelated"))))
          (is (= 2 (count @exports)))))
      (testing "a manifest that would let both refs read empty stores is rejected"
        (doseq [[problem change] [["malformed JSON" (constantly "{\"cwd\":")]
                                  ["no env" #(dissoc % "env")]
                                  ["an empty env" #(assoc % "env" {})]
                                  ["no seeded variables" #(dissoc % "seeded")]
                                  ["an empty seeded list" #(assoc % "seeded" [])]
                                  ["a seeded variable missing from env" #(update % "env" dissoc "CLAUDE_CONFIG_DIR")]
                                  ["a seeded variable naming no existing store"
                                   #(assoc-in % ["env" "CLAUDE_CONFIG_DIR"] (system/path dir "gone"))]
                                  ["a missing working directory" #(assoc % "cwd" (system/path dir "gone"))]
                                  ["no window" #(dissoc % "window")]
                                  ["a malformed window bound" #(assoc-in % ["window" "until"] "2026-01-01")]]
                :let [fixture (system/path dir (str "invalid-" (clojure.string/replace problem " " "-")))]]
          (write-manifest! fixture (change (golden-manifest fixture)))
          (is (thrown-with-msg? clojure.lang.ExceptionInfo #"is not a usable golden fixture"
                                (diff/prepare-golden-fixture! "/repo" fixture))
              problem)))
      (finally (babashka.fs/delete-tree dir)))))

(defn- run [exit out err] {:exit exit :out (.getBytes out) :err (.getBytes err)})

(deftest classification
  (let [base (run 0 "a" "w")
        verdict (fn [runs allow-stderr-diff] (select-keys (diff/classify runs allow-stderr-diff) [:status :streams]))]
    (is (= {:status :identical :streams []} (verdict {:base [base (run 0 "a" "w")] :head [(run 0 "a" "w")]} false)))
    (is (= {:status :different :streams [:stdout]} (verdict {:base [base base] :head [(run 0 "b" "w")]} false)))
    (is (= {:status :different :streams [:exit :stderr]} (verdict {:base [base base] :head [(run 1 "a" "x")]} false)))
    (testing "stderr is ignored on request, the exit code never"
      (is (= :identical (:status (diff/classify {:base [base base] :head [(run 0 "a" "x")]} true))))
      (is (= {:status :different :streams [:exit]} (verdict {:base [base base] :head [(run 1 "a" "x")]} true))))
    (testing "matching nonzero exits fail even when stderr differences are allowed"
      (doseq [exit [1 2 130] allow-stderr-diff [false true]
              :let [failed (run exit "" "rejected")]]
        (is (= {:status :failed :streams [:exit]}
               (verdict {:base [failed failed] :head [failed failed]} allow-stderr-diff)))))
    (testing "a base that disagrees with itself is unstable, whatever head printed"
      (is (= {:status :unstable :streams [:stdout]} (verdict {:base [base (run 0 "c" "w")] :head [(run 0 "b" "w")]} false)))
      (is (= {:status :unstable :streams [:stdout]}
             (verdict {:base [base base base (run 0 "c" "w")] :head [(run 0 "b" "w") base base]} false)))
      (is (= :identical (:status (diff/classify {:base [base (run 0 "a" "y")] :head [base]} true)))))
    (testing "a head that only sometimes disagrees with a reproducible base is still different"
      (let [odd (run 0 "b" "w")
            result (diff/classify {:base [base base base base] :head [base base odd]} false)]
        (is (= :different (:status result)))
        (is (= [base odd] (:pair result)))))))

(deftest summary
  (is (= {"Commands run" 5 "Identical" 2 "Different" 1 "Unstable" 1 "Failed" 1}
         (diff/summary-counts [{:status :identical} {:status :different} {:status :identical} {:status :unstable} {:status :failed}]))))

(deftest end-to-end
  (let [dir (babashka.fs/create-temp-dir {:prefix "ccusage-diff-e2e."})
        script (fn [name body] (stub-binary dir name (str "[ \"$1\" = --help ] && printf 'COMMANDS:\\n  daily  Show\\n  weekly  Show\\n' && exit 0\n"
                                                            "[ \"$2\" = --help ] && exit 0\n" body)))
        fixture (doto (system/path dir "fixture") (write-manifest! (golden-manifest dir)))
        stable (script "stable" "echo '{\"a\":1,\"b\":2}'")
        rejected (script "rejected" "echo 'unsupported report arguments' >&2; exit 2")
        unlaunchable (doto (script "unlaunchable" "echo unused")
                       (babashka.fs/set-posix-file-permissions "rw-------"))
        reordered (script "reordered" "echo '{\"b\":2,\"a\":1}'")
        changed (script "changed" "echo '{\"b\":2,\"a\":3}'; echo warn >&2")
        mixed-head (script "mixed-head" "case \"$1\" in weekly) echo '{\"a\":2,\"b\":2}';; *) echo '{\"a\":1,\"b\":2}';; esac")
        intermittent (script "intermittent"
                             (str "if [ -f " (system/path dir "head-ran") " ]; then echo '{\"a\":3,\"b\":2}'; "
                                  "else : > " (system/path dir "head-ran") "; echo '{\"a\":1,\"b\":2}'; fi"))
        flaky (script "flaky" (str "n=0; if [ -f " (system/path dir "calls") " ]; then read -r n < " (system/path dir "calls")
                                   "; fi; n=$((n + 1)); echo \"$n\" > " (system/path dir "calls") "; echo \"$n\""))
        compare (fn [bins allow-stderr-diff]
                  (diff/compare-command! bins (diff/golden-env {"env" {}} true) (str dir) (str dir) allow-stderr-diff
                                    {:id "daily--json" :json? true :args ["daily"]}))]
    (try
      (testing "identical output leaves nothing behind"
        (let [result (compare {:base stable :head stable} false)]
          (is (= :identical (:status result)))
          (is (not (babashka.fs/exists? (:dir result))))))
      (testing "matching rejected commands cannot pass a report comparison"
        (let [output-dir (system/path dir "rejected-reports")
              status (atom nil)
              output (with-out-str
                       (reset! status (diff/diff-main rejected rejected "--fixture" fixture "--output-dir" output-dir)))]
          (is (= 2 @status))
          (is (not (re-find #"(?m)^PASS\s" output)))
          (is (re-find #"(?m)^ERROR\s" output))
          (is (= "unsupported report arguments\n"
                 (slurp (system/path output-dir "daily--json" "base-1.stderr"))))))
      (testing "a command execution exception is an inconclusive comparison"
        (let [entry (system/expand (system/path script-dir "diff-refs.bb"))
              result (system/run-process [entry "--fixture" fixture "--output-dir" (system/path dir "unlaunchable-reports") unlaunchable stable])]
          (is (= 2 (:exit result)))
          (is (re-find #"(?m)^ERROR\s" (:out result)))
          (is (not (re-find #"(?m)^FAIL\s" (:out result))))
          (is (re-find #"IOException: [^\n]+" (:out result)))
          (is (.contains (:out result) unlaunchable))))
      (testing "invalid comparison arguments are not reported as output differences"
        (let [result (system/run-process [(system/expand (system/path script-dir "diff-refs.bb")) "--jobs" "0" "HEAD" "HEAD"])]
          (is (= 2 (:exit result)))
          (is (clojure.string/blank? (:out result)))))
      (testing "an incomplete command takes precedence over confirmed differences"
        (let [output-dir (system/path dir "mixed-results")
              _ (babashka.fs/create-dirs output-dir)
              _ (spit (system/path output-dir "daily--json") "blocks a command output directory")
              result (system/run-process [(system/expand (system/path script-dir "diff-refs.bb")) "--fixture" fixture "--output-dir" output-dir stable mixed-head])]
          (is (= 2 (:exit result)))
          (is (re-find #"(?m)^ERROR\s" (:out result)))
          (is (re-find #"(?m)^FAIL\s" (:out result)))))
      (testing "head must agree on repeated runs even when its first run matches"
        (let [result (compare {:base stable :head intermittent} false)]
          (is (= :different (:status result)))
          (is (= [:stdout] (:streams result)))))
      (testing "a difference keeps both outputs and diffs them key-sorted"
        (let [result (compare {:base stable :head changed} false)
              [base head] (:pair result)
              diff (diff/unified-diff (:out-file base) (:out-file head) true)]
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
              diff (diff/unified-diff (:out-file base) (:out-file head) true)]
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
      (with-redefs [history/resolve-variants (fn [& _] [{:bin "base"} {:bin "head"}])
                    diff/prepare-golden-fixture! (fn [& _] {"cwd" (str dir) "env" {}})
                    diff/report-commands (fn [_] ["daily"])
                    diff/run-all! (fn [& _] [{:status :unstable :args ["daily"]}])
                    system/write-progress (fn [_])]
        (with-out-str
          (reset! status (diff/diff-main "HEAD" "HEAD" "--output-dir" (str dir)))))
      (is (= 2 @status))
      (finally (babashka.fs/delete-tree dir)))))

(let [{:keys [fail error]} (apply run-tests ['diff-refs-test])]
  (when (pos? (+ fail error))
    (System/exit 1)))
