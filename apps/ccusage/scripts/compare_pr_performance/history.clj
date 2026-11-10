(def history-spec
  {:fixture-dir {:ref "DIR" :desc "Claude fixture; pass with --codex-fixture-dir, or omit both to generate them"}
   :codex-fixture-dir {:ref "DIR" :desc "Codex fixture; pass with --fixture-dir"}
   :size-mib {:coerce :long :default 1024 :ref "N" :desc "Size of each generated fixture"}
   :real {:coerce :boolean :desc "Also measure the logs this environment points at (default dirs, CLAUDE_CONFIG_DIR, CODEX_HOME)"}
   :runs {:coerce :long :default 5 :ref "N" :desc "Interleaved measured rounds; at least 3"}
   :warmup {:coerce :long :default 1 :ref "N"}
   :threshold {:coerce :double :default 0.1 :ref "RATIO"}
   :commands {:ref "LIST" :desc "Comma-separated report commands"}
   :cache-dir {:ref "DIR" :desc "Built binaries, shared cargo target, and generated fixtures (default: rust/target/bench-history)"}
   :output {:ref "FILE"}
   :help {:alias :h :coerce :boolean}})

(defn local-logs-env []
  {:clean? false :vars (array-map "COLUMNS" "200" "LOG_LEVEL" "0" "NO_COLOR" "1" "TZ" "UTC")})

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
                             "Instructions vs prev" (if (:instructions comparison) (format-ratio-spread (:instructions comparison)) "-")
                             "Verdict" (if comparison (comparison-verdict comparison) "-"))))
              variants)
        rows (if (some #(not= "-" (get % "Instructions vs prev")) (rest rows))
               rows
               (map #(dissoc % "Instructions vs prev") rows))]
    (str "### `" command " --offline --json`\n\n" (table-md rows))))

(defn parse-history-cli [argv]
  (let [{:keys [args opts]} (cli/parse-args argv {:spec history-spec :restrict true})]
    (when-not (:help opts)
      (when (empty? args)
        (fail! "Pass at least one git ref or ccusage binary path" {:kind :invalid-cli}))
      (when-let [duplicates (seq (for [[ref n] (frequencies args) :when (> n 1)] ref))]
        (fail! (str "Each ref or binary may appear once: " (str/join ", " duplicates)) {:kind :invalid-cli}))
      (when (not= (some? (:fixture-dir opts)) (some? (:codex-fixture-dir opts)))
        (fail! "Pass --fixture-dir and --codex-fixture-dir together" {:kind :invalid-cli}))
      (assert-sample-options opts))
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

(import '[java.nio.channels FileChannel]
        '[java.nio.file OpenOption StandardOpenOption])

(defn with-file-lock
  "Runs f while holding an exclusive lock on file, so runs sharing the cache
  take turns instead of interleaving writes."
  [file f]
  (fs/create-dirs (fs/parent file))
  (with-open [channel (FileChannel/open (fs/path file)
                                        (into-array OpenOption [StandardOpenOption/CREATE StandardOpenOption/WRITE]))]
    ;; Closing the channel at the end of with-open releases the lock.
    (.lock channel)
    (f)))

(defn- build-commit-locked! [repo cache commit worktree bin]
  (when-not (fs/exists? bin)
    (when-not (fs/exists? worktree)
      (run-checked ["git" "worktree" "add" "--detach" "--quiet" worktree commit] {:dir repo} "git worktree add"))
    (run-checked ["git" "-C" worktree "checkout" "--detach" "--quiet" commit] {} "git checkout")
    (write-progress (str "building " (format-sha commit)))
    (run-checked ["cargo" "build" "--manifest-path" (path worktree "rust" "Cargo.toml") "--release" "--bin" "ccusage"]
                 {:env {"CARGO_TARGET_DIR" (path cache "target")}} (str "cargo build " (format-sha commit)))
    (fs/create-dirs (fs/parent bin))
    (let [partial (str bin ".partial")]
      (fs/copy (path cache "target" "release" "ccusage") partial {:copy-attributes true :replace-existing true})
      (fs/move partial bin {:atomic-move true :replace-existing true}))))

(defn build-commit!
  "Builds one commit into the cache, reusing a binary built earlier. A shared
  target directory keeps dependency artifacts across refs, so building and
  copying its binary out happen under one lock."
  [repo cache commit worktree]
  (let [bin (path cache "bin" commit "ccusage")]
    (when-not (fs/exists? bin)
      (with-file-lock (path cache "build.lock") #(build-commit-locked! repo cache commit worktree bin)))
    bin))

(defn resolve-variants
  "Each run checks refs out into its own temporary worktree, so concurrent runs
  sharing the cache cannot remove each other's checkout."
  [repo cache refs]
  (let [scratch (temp-dir "ccusage-bench-src.")
        worktree (path scratch "src")]
    (try
      (mapv (fn [ref]
              (if (fs/regular-file? ref)
                {:label ref :bin (expand ref)}
                (let [commit (resolve-commit repo ref)]
                  {:label ref :commit commit :bin (build-commit! repo cache commit worktree)})))
            refs)
      (finally
        (when (fs/exists? worktree)
          (run-process ["git" "worktree" "remove" "--force" worktree] {:dir repo}))
        (fs/delete-tree scratch)))))

(defn file-sha256 [file]
  (let [digest (java.security.MessageDigest/getInstance "SHA-256")]
    (format "%064x" (BigInteger. 1 (.digest digest (fs/read-all-bytes file))))))

(def codex-input-classes
  "Codex inputs that older releases skip, from the Codex loader's history
  (`rust/adapters/codex/src/paths.rs` and `reader.rs`): each entry names the
  commit that taught the reader that input. Claude discovery has scanned
  `projects/` recursively throughout v20, and date-window skips (#1665,
  #1794) do not apply to unbounded reports, so nothing else belongs here."
  [{:name "archived sessions (`archived_sessions/`)" :pr "#1230"
    :commit "a9b12c43ac608d308bb4a2c5f5673dc6b9e530c1"
    :matches? #(str/includes? % "/archived_sessions/")}
   {:name "zstd-compressed rollouts (`.jsonl.zst`)" :pr "#1890"
    :commit "44a3aa84e84cbc1afa40dab3f2849e03e9a9b054"
    :matches? #(str/ends-with? % ".zst")}])

(defn reads-input-class?
  "A binary path cannot be checked, so only a commit that contains the
  class's reader support counts as reading it."
  [repo {:keys [commit]} input-class]
  (and (some? commit)
       (zero? (:exit (run-process ["git" "merge-base" "--is-ancestor" (:commit input-class) commit] {:dir repo})))))

(defn- refs-text [variants] (str/join ", " (map #(str "`" (:label %) "`") variants)))

(defn codex-compatibility
  "Picks the Codex layout every ref reads in full, and the report note that
  goes with it. A ref that skips part of the input would otherwise look
  faster, or its successor slower, only because the input it read changed.
  `reads?` takes a variant and an input class."
  [reads? variants fixture-codex-dir]
  (let [skipped (keep (fn [input-class]
                        (when-let [refs (seq (remove #(reads? % input-class) variants))]
                          (assoc input-class :refs refs)))
                      codex-input-classes)
        describe (fn [classes] (str/join "; " (map #(format "%s cannot read %s (added in %s)" (refs-text (:refs %)) (:name %) (:pr %)) classes)))]
    (cond
      (empty? skipped) {:layout "current"}
      (nil? fixture-codex-dir)
      {:layout "legacy"
       :note (str "Note: " (describe skipped) ". The Codex fixture therefore uses the legacy layout: every rollout "
                  "uncompressed under `sessions/`, which every ref reads in full.")}
      :else
      (let [files (map str (filter fs/regular-file? (file-seq (fs/file fixture-codex-dir))))
            present (filter (fn [input-class] (some (:matches? input-class) files)) skipped)]
        (when (seq present)
          {:warning (str "⚠️ **Warning:** the custom Codex fixture contains input some refs skip: " (describe present)
                         ". Those refs read less input, so their Codex and unified rows are not comparable. "
                         "Omit both fixture options to get a fixture every ref reads.")})))))

(defn ensure-fixture!
  "Generates the fixture once per generator revision, size, and Codex layout."
  [repo cache size-mib layout]
  (let [generator (path repo "apps" "ccusage" "scripts" "generate-large-fixture.ts")
        dir (path cache "fixture" (str size-mib "-" layout "-" (subs (file-sha256 generator) 0 12)))
        claude (path dir "claude") codex (path dir "codex") done (path dir ".complete")]
    (with-file-lock (str dir ".lock")
      #(when-not (fs/exists? done)
         (fs/delete-tree dir)
         (write-progress (str "generating " size-mib " MiB fixtures in " dir))
         (run-checked [generator "--output-dir" claude "--codex-output-dir" codex
                       "--size-mib" (str size-mib) "--codex-size-mib" (str size-mib) "--codex-layout" layout]
                      {:dir repo} "generate-large-fixture")
         (spit done "")))
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
            custom-codex (some-> (:codex-fixture-dir options) expand)
            compatibility (codex-compatibility (partial reads-input-class? repo) variants custom-codex)
            fixture (if custom-codex
                      {:claude (expand (:fixture-dir options)) :codex custom-codex}
                      (ensure-fixture! repo cache (:size-mib options) (:layout compatibility)))
            empty-home (temp-dir "ccusage-perf-home.")
            stats (fn [dir] (let [s (summarize-directory dir)] (format "%s, %d files" (format-data-size (:bytes s)) (:files s))))]
        (try
          (let [synthetic (history-section (format "Synthetic fixture (Claude %s; Codex %s)" (stats (:claude fixture)) (stats (:codex fixture)))
                                           variants (fixture-env (:claude fixture) (:codex fixture) (str empty-home)) commands options)
                real (when (:real options)
                       (history-section "Local logs" variants (local-logs-env) commands options))
                real-warning (when (and real (not= "current" (:layout compatibility)))
                               (str "⚠️ **Warning:** in local logs, "
                                    (str/join "; " (keep (fn [input-class]
                                                           (when-let [refs (seq (remove #(reads-input-class? repo % input-class) variants))]
                                                             (format "%s skip %s" (refs-text refs) (:name input-class))))
                                                         codex-input-classes))
                                    ", so Codex and unified rows across those changes compare different input.\n\n"))
                report (str "# ccusage performance history\n\n"
                            (some-> (or (:warning compatibility) (:note compatibility)) (str "\n\n"))
                            (format "%d interleaved rounds after %d warmup rounds; cells show median (min–max); ratios are this ref / previous ref per round. "
                                    (:runs options) (:warmup options))
                            "\n\n" synthetic (when real (str "\n" real-warning real)))]
            (write-output! (:output options) report))
          (finally (fs/delete-tree empty-home)))))))
