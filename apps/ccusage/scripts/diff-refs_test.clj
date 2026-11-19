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

(defn- stub-binary
  "An executable /bin/sh script in `dir` running `body`."
  [dir name body]
  (let [file (path dir name)]
    (spit file (str "#!/bin/sh\nPATH=/no-such-path\n" body "\n"))
    (babashka.fs/set-posix-file-permissions file "rwx------")
    file))

(def stub-help
  "Answers `--help` like the CLI: COMMANDS at the root and for each agent,
  options only for a report."
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
               (report-commands (stub-binary dir "cli" stub-help)))))
      (testing "a help page without commands cannot yield an empty plan"
        (is (thrown-with-msg? clojure.lang.ExceptionInfo #"lists no report commands"
                              (report-commands (stub-binary dir "bare" "echo 'OPTIONS:'")))))
      (finally (babashka.fs/delete-tree dir)))))

(def planned ["daily" "weekly" "blocks" "claude daily" "claude blocks" "codex session"])

(defn- args-of [plan id] (:args (first (filter #(= id (:id %)) plan))))

(deftest planned-commands
  (let [fixture (command-plan {} {:since "20250101" :until "20250131"} planned)
        real (command-plan {:real true} (real-dates (java.time.LocalDate/of 2026 3 1)) planned)]
    (is (= (* (count output-variants) (count planned)) (count fixture) (count real)))
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
  (let [fixture (command-plan {} {:since "20250101" :until "20250131"} planned)
        real (command-plan {:real true} (real-dates (java.time.LocalDate/of 2026 3 1)) planned)]
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
  (let [env (golden-env pinned-manifest true)]
    (is (true? (:clean? env)))
    (testing "the golden environment at the default log level"
      (is (= {"HOME" "/f/home" "CLAUDE_CONFIG_DIR" "/f/claude,/f/opencode/claude"
              "CCUSAGE_TEST_NOW" "2026-01-02T12:00:00Z" "COLUMNS" "200" "NO_COLOR" "1" "TZ" "UTC"}
             (:vars env))))
    (testing "nothing from the caller leaks into a fixture run"
      (is (= (:vars env) (process-env env {"CODEX_HOME" "/real" "PATH" "/bin"})))))
  (testing "binaries that cannot pin the clock both run under the wall clock"
    (is (not (contains? (:vars (golden-env pinned-manifest false)) "CCUSAGE_TEST_NOW"))))
  (testing "a real run inherits the caller's environment at the default log level and the wall clock"
    (is (= {"HOME" "/h" "CODEX_HOME" "/real" "COLUMNS" "200" "NO_COLOR" "1" "TZ" "UTC"}
           (process-env (real-env) {"HOME" "/h" "CODEX_HOME" "/real" "LOG_LEVEL" "0" "COLUMNS" "80"
                                    "CCUSAGE_TEST_NOW" "2026-01-02T12:00:00Z"})))))

(deftest pinned-clock-builds
  (testing "a ref is built with the test clock only when its ccusage-core declares it"
    (is (true? (declares-test-clock? "[features]\ntest-snapshots = [\"dep:x\"]\ntest-clock = []\n")))
    (is (false? (declares-test-clock? "[features]\ntest-snapshots = [\"dep:x\"]\n")))
    (is (false? (declares-test-clock? nil))))
  (testing "this checkout declares it, so its own goldens' clock is honored"
    (is (= [test-clock-feature] (clock-features (repo-root) "HEAD"))))
  (testing "a test-clock build is cached apart from the plain release build"
    (is (= "abc" (build-key "abc" [])))
    (is (not= (build-key "abc" []) (build-key "abc" [test-clock-feature])))))

(defn- run-diff-main
  "diff-main over stub variants, returning the environment each command ran
  under and the progress messages."
  [variants]
  (let [dir (babashka.fs/create-temp-dir {:prefix "ccusage-diff-clock."})
        envs (atom []) progress (atom []) features-for (atom nil)]
    (try
      (with-redefs [resolve-variants (fn [_ _ _ f] (reset! features-for f) variants)
                    prepare-golden-fixture! (fn [& _] (assoc pinned-manifest "cwd" (str dir)))
                    report-commands (fn [_] ["daily"])
                    compare-command! (fn [_ env & _] (swap! envs conj env) {:status :identical :args ["daily"]})
                    write-progress #(swap! progress conj %)]
        (with-out-str (diff-main "HEAD" "HEAD" "--output-dir" (str dir))))
      {:envs @envs :progress @progress :features-for @features-for}
      (finally (babashka.fs/delete-tree dir)))))

(deftest pinned-clock-comparisons
  (testing "refs are built with the test clock where they declare it"
    (let [features-for (:features-for (run-diff-main [{:bin "b" :features []} {:bin "h" :features []}]))]
      (is (= [test-clock-feature] (features-for "HEAD")))))
  (testing "both sides pinned keep the golden clock"
    (let [{:keys [envs]} (run-diff-main [{:bin "b" :features [test-clock-feature]} {:bin "h" :features [test-clock-feature]}])]
      (is (seq envs))
      (is (every? #(= "2026-01-02T12:00:00Z" (get-in % [:vars "CCUSAGE_TEST_NOW"])) envs))))
  (testing "one side that cannot pin it moves both to the wall clock, with a notice naming it"
    (let [{:keys [envs progress]} (run-diff-main [{:label "v18.0.0" :bin "b" :features []}
                                                  {:label "HEAD" :bin "h" :features [test-clock-feature]}])]
      (is (seq envs))
      (is (not-any? #(contains? (:vars %) "CCUSAGE_TEST_NOW") envs))
      (is (some #(and (.contains ^String % "v18.0.0") (.contains ^String % "wall clock")) progress)))))

(defn- golden-manifest
  "What golden::export_fixture writes into `dir`, with a seeded store there."
  [dir]
  (let [store (doto (path dir "claude") babashka.fs/create-dirs)]
    {"cwd" (str dir)
     "env" {"HOME" (str dir) "CLAUDE_CONFIG_DIR" (str " " store " ," (path dir "missing"))}
     "seeded" ["CLAUDE_CONFIG_DIR"]
     "window" {"since" "20251231" "until" "20260101"}}))

(defn- write-manifest! [dir manifest]
  (babashka.fs/create-dirs dir)
  (spit (path dir fixture-manifest) (if (string? manifest) manifest (cheshire.core/generate-string manifest))))

(deftest golden-fixtures
  (let [dir (babashka.fs/create-temp-dir {:prefix "ccusage-diff-fixture."})
        exports (atom [])]
    (try
      (with-redefs [export-golden-fixture! (fn [_ target]
                                             (swap! exports conj target)
                                             (write-manifest! target (golden-manifest target)))]
        (testing "a missing or empty directory is exported into, an exported one reused"
          (let [exported (prepare-golden-fixture! "/repo" (path dir "new"))]
            (is (= (golden-manifest (path dir "new")) exported (prepare-golden-fixture! "/repo" (path dir "new")))))
          (babashka.fs/create-dirs (path dir "empty"))
          (prepare-golden-fixture! "/repo" (path dir "empty"))
          (is (= [(path dir "new") (path dir "empty")] @exports)))
        (testing "a directory holding something else is never written into"
          (babashka.fs/create-dirs (path dir "unrelated"))
          (spit (path dir "unrelated" "notes") "keep")
          (is (thrown-with-msg? clojure.lang.ExceptionInfo #"is not a golden fixture"
                                (prepare-golden-fixture! "/repo" (path dir "unrelated"))))
          (is (= 2 (count @exports)))))
      (testing "a manifest that would let both refs read empty stores is rejected"
        (doseq [[problem change] [["malformed JSON" (constantly "{\"cwd\":")]
                                  ["no env" #(dissoc % "env")]
                                  ["an empty env" #(assoc % "env" {})]
                                  ["no seeded variables" #(dissoc % "seeded")]
                                  ["an empty seeded list" #(assoc % "seeded" [])]
                                  ["a seeded variable missing from env" #(update % "env" dissoc "CLAUDE_CONFIG_DIR")]
                                  ["a seeded variable naming no existing store"
                                   #(assoc-in % ["env" "CLAUDE_CONFIG_DIR"] (path dir "gone"))]
                                  ["a missing working directory" #(assoc % "cwd" (path dir "gone"))]
                                  ["no window" #(dissoc % "window")]
                                  ["a malformed window bound" #(assoc-in % ["window" "until"] "2026-01-01")]]
                :let [fixture (path dir (str "invalid-" (clojure.string/replace problem " " "-")))]]
          (write-manifest! fixture (change (golden-manifest fixture)))
          (is (thrown-with-msg? clojure.lang.ExceptionInfo #"is not a usable golden fixture"
                                (prepare-golden-fixture! "/repo" fixture))
              problem)))
      (finally (babashka.fs/delete-tree dir)))))

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
    (testing "matching nonzero exits fail even when stderr differences are allowed"
      (doseq [exit [1 2 130] allow-stderr-diff [false true]
              :let [failed (run exit "" "rejected")]]
        (is (= {:status :failed :streams [:exit]}
               (verdict {:base [failed failed] :head [failed failed]} allow-stderr-diff)))))
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
  (is (= {"Commands run" 5 "Identical" 2 "Different" 1 "Unstable" 1 "Failed" 1}
         (summary-counts [{:status :identical} {:status :different} {:status :identical} {:status :unstable} {:status :failed}]))))

(deftest end-to-end
  (let [dir (babashka.fs/create-temp-dir {:prefix "ccusage-diff-e2e."})
        script (fn [name body] (stub-binary dir name (str "[ \"$1\" = --help ] && printf 'COMMANDS:\\n  daily  Show\\n  weekly  Show\\n' && exit 0\n"
                                                            "[ \"$2\" = --help ] && exit 0\n" body)))
        fixture (doto (path dir "fixture") (write-manifest! (golden-manifest dir)))
        stable (script "stable" "echo '{\"a\":1,\"b\":2}'")
        rejected (script "rejected" "echo 'unsupported report arguments' >&2; exit 2")
        unlaunchable (doto (script "unlaunchable" "echo unused")
                       (babashka.fs/set-posix-file-permissions "rw-------"))
        reordered (script "reordered" "echo '{\"b\":2,\"a\":1}'")
        changed (script "changed" "echo '{\"b\":2,\"a\":3}'; echo warn >&2")
        mixed-head (script "mixed-head" "case \"$1\" in weekly) echo '{\"a\":2,\"b\":2}';; *) echo '{\"a\":1,\"b\":2}';; esac")
        intermittent (script "intermittent"
                             (str "if [ -f " (path dir "head-ran") " ]; then echo '{\"a\":3,\"b\":2}'; "
                                  "else : > " (path dir "head-ran") "; echo '{\"a\":1,\"b\":2}'; fi"))
        flaky (script "flaky" (str "n=0; if [ -f " (path dir "calls") " ]; then read -r n < " (path dir "calls")
                                   "; fi; n=$((n + 1)); echo \"$n\" > " (path dir "calls") "; echo \"$n\""))
        compare (fn [bins allow-stderr-diff]
                  (compare-command! bins (golden-env {"env" {}} true) (str dir) (str dir) allow-stderr-diff
                                    {:id "daily--json" :json? true :args ["daily"]}))]
    (try
      (testing "identical output leaves nothing behind"
        (let [result (compare {:base stable :head stable} false)]
          (is (= :identical (:status result)))
          (is (not (babashka.fs/exists? (:dir result))))))
      (testing "matching rejected commands cannot pass a report comparison"
        (let [output-dir (path dir "rejected-reports")
              status (atom nil)
              output (with-out-str
                       (reset! status (diff-main rejected rejected "--fixture" fixture "--output-dir" output-dir)))]
          (is (= 2 @status))
          (is (not (re-find #"(?m)^PASS\s" output)))
          (is (re-find #"(?m)^ERROR\s" output))
          (is (= "unsupported report arguments\n"
                 (slurp (path output-dir "daily--json" "base-1.stderr"))))))
      (testing "a command execution exception is an inconclusive comparison"
        (let [entry (expand (path script-dir "diff-refs.bb"))
              result (run-process [entry "--fixture" fixture "--output-dir" (path dir "unlaunchable-reports") unlaunchable stable])]
          (is (= 2 (:exit result)))
          (is (re-find #"(?m)^ERROR\s" (:out result)))
          (is (not (re-find #"(?m)^FAIL\s" (:out result))))
          (is (.contains (:out result) "IOException"))))
      (testing "invalid comparison arguments are not reported as output differences"
        (let [result (run-process [(expand (path script-dir "diff-refs.bb")) "--jobs" "0" "HEAD" "HEAD"])]
          (is (= 2 (:exit result)))
          (is (clojure.string/blank? (:out result)))))
      (testing "an incomplete command takes precedence over confirmed differences"
        (let [output-dir (path dir "mixed-results")
              _ (babashka.fs/create-dirs output-dir)
              _ (spit (path output-dir "daily--json") "blocks a command output directory")
              result (run-process [(expand (path script-dir "diff-refs.bb")) "--fixture" fixture "--output-dir" output-dir stable mixed-head])]
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
                    prepare-golden-fixture! (fn [& _] {"cwd" (str dir) "env" {}})
                    report-commands (fn [_] ["daily"])
                    run-all! (fn [& _] [{:status :unstable :args ["daily"]}])
                    write-progress (fn [_])]
        (with-out-str
          (reset! status (diff-main "HEAD" "HEAD" "--output-dir" (str dir)))))
      (is (= 2 @status))
      (finally (babashka.fs/delete-tree dir)))))

(let [{:keys [fail error]} (apply run-tests ['diff-refs-test])]
  (when (pos? (+ fail error))
    (System/exit 1)))
