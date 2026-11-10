(def history-spec
  {:fixture-dir {:ref "DIR" :desc "Claude fixture; generated under the cache when omitted"}
   :codex-fixture-dir {:ref "DIR" :desc "Codex fixture; generated under the cache when omitted"}
   :size-mib {:coerce :long :default 1024 :ref "N" :desc "Size of each generated fixture"}
   :real {:coerce :boolean :desc "Also measure the logs this environment points at (default dirs, CLAUDE_CONFIG_DIR, CODEX_HOME)"}
   :runs {:coerce :long :default 5 :ref "N"}
   :warmup {:coerce :long :default 1 :ref "N"}
   :threshold {:coerce :double :default 0.1 :ref "RATIO"}
   :commands {:ref "LIST" :desc "Comma-separated report commands"}
   :cache-dir {:ref "DIR" :desc "Built binaries, shared cargo target, and generated fixtures (default: rust/target/bench-history)"}
   :output {:ref "FILE"}
   :help {:alias :h :coerce :boolean}})

(defn local-logs-env []
  (array-map "COLUMNS" "200" "LOG_LEVEL" "0" "NO_COLOR" "1" "TZ" "UTC"))

(defn render-history-table
  "One table per command: every ref against the previous one, judged like a PR."
  [{:keys [command rounds]} variants threshold]
  (let [rows (map-indexed
              (fn [index {:keys [label commit]}]
                (let [summary (variant-summary rounds label)
                      previous (when (pos? index) (:label (nth variants (dec index))))
                      comparison (when previous (compare-variants rounds previous label threshold))]
                  (array-map "Ref" label "Commit" (if commit (str "`" (format-sha commit) "`") "-")
                             "CPU" (format-spread (:cpu summary) format-duration)
                             "Wall" (format-spread (:wall summary) format-duration)
                             "Peak RSS" (format-memory-size (get-in summary [:rss :median]))
                             "CPU vs prev" (if comparison (format-ratio-spread (:cpu comparison)) "-")
                             "RSS vs prev" (if comparison (format-ratio-spread (:rss comparison)) "-")
                             "Verdict" (if comparison (comparison-verdict comparison) "-"))))
              variants)]
    (str "### `" command " --offline --json`\n\n" (table-md rows))))

(defn parse-history-cli [argv]
  (let [{:keys [args opts]} (cli/parse-args argv {:spec history-spec :restrict true})]
    (when-not (:help opts)
      (when (empty? args)
        (fail! "Pass at least one git ref or ccusage binary path" {:kind :invalid-cli}))
      (assert-sample-options (assoc opts :runs (max 3 (:runs opts)))))
    (assoc opts :refs (vec args))))

(defn history-help-text []
  (str "Usage: bench-history.bb [options] <ref|binary>...\n\n"
       "Builds each git ref in release mode (or takes an existing binary path), then runs every\n"
       "report command over the same fixture in interleaved rounds and compares each ref with the\n"
       "previous one.\n\n"
       (cli/format-opts {:spec history-spec})))

(defn repo-root [] (str/trim (:out (run-checked ["git" "rev-parse" "--show-toplevel"] {} "git rev-parse"))))

(defn resolve-commit [repo ref]
  (str/trim (:out (run-checked ["git" "rev-parse" "--verify" "--quiet" (str ref "^{commit}")] {:dir repo}
                               (str "Unknown git ref " ref)))))

(defn build-commit!
  "Builds one commit into the cache, reusing a binary built earlier. A shared
  target directory keeps dependency artifacts across refs."
  [repo cache commit worktree]
  (let [bin (path cache "bin" commit "ccusage")]
    (when-not (fs/exists? bin)
      (when-not (fs/exists? worktree)
        (run-checked ["git" "worktree" "add" "--detach" "--quiet" worktree commit] {:dir repo} "git worktree add"))
      (run-checked ["git" "-C" worktree "checkout" "--detach" "--quiet" commit] {} "git checkout")
      (write-progress (str "building " (format-sha commit)))
      (run-checked ["cargo" "build" "--manifest-path" (path worktree "rust" "Cargo.toml") "--release" "--bin" "ccusage"]
                   {:env {"CARGO_TARGET_DIR" (path cache "target")}} (str "cargo build " (format-sha commit)))
      (fs/create-dirs (fs/parent bin))
      (fs/copy (path cache "target" "release" "ccusage") bin))
    bin))

(defn resolve-variants [repo cache refs]
  (let [worktree (path cache "src")]
    (try
      (mapv (fn [ref]
              (if (fs/regular-file? ref)
                {:label (str (fs/file-name ref)) :bin (expand ref)}
                (let [commit (resolve-commit repo ref)]
                  {:label ref :commit commit :bin (build-commit! repo cache commit worktree)})))
            refs)
      (finally
        (when (fs/exists? worktree)
          (run-process ["git" "worktree" "remove" "--force" worktree] {:dir repo}))))))

(defn file-sha256 [file]
  (let [digest (java.security.MessageDigest/getInstance "SHA-256")]
    (format "%064x" (BigInteger. 1 (.digest digest (fs/read-all-bytes file))))))

(defn ensure-fixture!
  "Generates the fixture once per generator revision and size."
  [repo cache size-mib]
  (let [generator (path repo "apps" "ccusage" "scripts" "generate-large-fixture.ts")
        dir (path cache "fixture" (str size-mib "-" (subs (file-sha256 generator) 0 12)))
        claude (path dir "claude") codex (path dir "codex") done (path dir ".complete")]
    (when-not (fs/exists? done)
      (write-progress (str "generating " size-mib " MiB fixtures in " dir))
      (run-checked [generator "--output-dir" claude "--codex-output-dir" codex
                    "--size-mib" (str size-mib) "--codex-size-mib" (str size-mib)]
                   {:dir repo} "generate-large-fixture")
      (spit done ""))
    {:claude claude :codex codex}))

(defn history-section [title variants env commands options]
  (str "## " title "\n\n"
       (str/join "\n\n" (map #(render-history-table (measure-command variants % env (assoc options :title title))
                                                    variants (:threshold options))
                             commands))
       "\n"))

(defn history-main [& argv]
  (let [options (parse-history-cli argv)]
    (if (:help options)
      (println (history-help-text))
      (let [repo (repo-root)
            cache (expand (or (:cache-dir options) (path repo "rust" "target" "bench-history")))
            commands (if (:commands options) (mapv str/trim (str/split (:commands options) #",")) default-commands)
            variants (resolve-variants repo cache (:refs options))
            fixture (if (:fixture-dir options)
                      {:claude (expand (:fixture-dir options)) :codex (expand (or (:codex-fixture-dir options) (:fixture-dir options)))}
                      (ensure-fixture! repo cache (:size-mib options)))
            empty-home (temp-dir "ccusage-perf-home.")
            stats (fn [dir] (let [s (summarize-directory dir)] (format "%s, %d files" (format-data-size (:bytes s)) (:files s))))]
        (try
          (let [synthetic (history-section (format "Synthetic fixture (Claude %s; Codex %s)" (stats (:claude fixture)) (stats (:codex fixture)))
                                           variants (fixture-env (:claude fixture) (:codex fixture) (str empty-home)) commands options)
                real (when (:real options)
                       (history-section "Local logs" variants (local-logs-env) commands options))
                report (str "# ccusage performance history\n\n"
                            (format "%d interleaved rounds after %d warmup rounds; cells show median (min–max); ratios are this ref / previous ref per round. "
                                    (:runs options) (:warmup options))
                            "\n\n" synthetic (when real (str "\n" real)))]
            (write-output! (:output options) report))
          (finally (fs/delete-tree empty-home)))))))
