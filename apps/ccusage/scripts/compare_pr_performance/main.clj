(defn write-output! [output markdown]
  (if output (spit (expand output) markdown) (print markdown)))

(defn prepare-context [options install-root]
  (let [base-dir (some-> (:base-dir options) expand) head-dir (expand (:head-dir options))
        base-install (when (:base-package-url options)
                       (install-package-url (path install-root "base-package") "base" (:base-package-url options) (:package-runner-timeout-ms options)))
        head-install (when (:head-package-url options)
                       (install-package-url (path install-root "head-package") "PR" (:head-package-url options) (:package-runner-timeout-ms options)))]
    (if (= :skip (base-source-decision {:package-install base-install :base-dir base-dir :base-package-url (:base-package-url options)}))
      {:skip-reason (str "Base package URL was not ready before " (format-duration (:package-runner-timeout-ms options))
                         ". Fixture performance comparison requires a base package when --base-dir is not provided.")
       :base-package-url (:base-package-url options) :base-sha (:base-sha options)
       :head-runtime (parse-head-runtime (:head-runtime options)) :head-sha (git-sha head-dir)}
      (let [base-native (when base-install (installed-native-package-bin-entry (path install-root "base-package")))
            head-native (when head-install (installed-native-package-bin-entry (path install-root "head-package")))
            runtime (parse-head-runtime (:head-runtime options))]
        (merge options
               {:base-dir base-dir :head-dir head-dir :fixture-dir (expand (:fixture-dir options))
                :codex-fixture-dir (expand (:codex-fixture-dir options))
                :large-fixture-dir (some-> (:large-fixture-dir options) expand)
                :large-codex-fixture-dir (some-> (:large-codex-fixture-dir options) expand)
                :base-package-install base-install :head-package-install head-install
                :base-bin-entry (if base-install (:bin-entry base-install) (package-bin-entry base-dir))
                :head-bin-entry (:bin-entry head-install) :base-native-bin-entry base-native :head-native-bin-entry head-native
                :base-runtime-description (when base-install "Base runs the published `ccusage` package from `pkg.pr.new`, installed before measurement")
                :head-runtime-description (cond (and (= runtime "rust") head-native) "PR runs the published native `ccusage` binary from `pkg.pr.new`, installed before measurement"
                                                (and (= runtime "package") head-install) "PR runs the published `ccusage` package from `pkg.pr.new`, installed before measurement")
                :base-sha (or (:base-sha options) (when base-dir (git-sha base-dir))) :head-sha (git-sha head-dir)})))))

(defn run-benchmark-suites [context]
  (let [common (select-keys context [:base-bin-entry :base-runtime-description :base-sha :head-bin-entry :head-dir
                                     :head-native-bin-entry :head-runtime :head-runtime-description :head-sha :memory-runs])
        committed (compare-fixture (merge common {:fixture-dir (:fixture-dir context) :codex-fixture-dir (:codex-fixture-dir context)
                                                   :commands ["claude daily" "claude session" "codex daily" "codex session"]
                                                   :description "Committed small fixtures for stable PR-to-PR feedback and explicit Claude/Codex command coverage."
                                                   :title "Committed fixture performance" :runs (:runs context) :warmup (:warmup context)}))
        large (when (:large-fixture-dir context)
                (compare-fixture (merge common {:fixture-dir (:large-fixture-dir context)
                                                :codex-fixture-dir (or (:large-codex-fixture-dir context) (:codex-fixture-dir context))
                                                :commands ["claude" "codex"]
                                                :description "Generated fixtures shaped from aggregate local log statistics: thousands of JSONL files, many small sessions, and a long tail of larger sessions. No real prompts, paths, or outputs are stored in the fixtures."
                                                :title "Large real-world-shaped fixture performance" :runs (:large-runs context) :warmup (:large-warmup context)})))]
    (cond-> [committed] large (conj large))))

(defn base-package-size-source [context]
  (if (:base-package-install context) :remote :local))

(defn head-package-size-source [context]
  (if (:head-package-install context) :remote :local))

(defn sizes-for [context]
  {:base-native-package-binary (optional-file-size (:base-native-bin-entry context))
   :base-package (if (= :remote (base-package-size-source context))
                   (remote-tarball-size (:base-package-url context))
                   (packed-tarball-size (:base-dir context)))
   :base-rust-binary (when (:base-dir context) (optional-file-size (rust-binary-entry (:base-dir context))))
   :head-native-package-binary (optional-file-size (:head-native-bin-entry context))
   :head-package (if (= :remote (head-package-size-source context))
                   (remote-tarball-size (:head-package-url context))
                   (packed-tarball-size (:head-dir context)))
   :head-rust-binary (optional-file-size (rust-binary-entry (:head-dir context)))})

(defn -main [& argv]
  (let [options (parse-cli argv)]
    (if (:help options)
      (println (help-text))
      (let [install-root (temp-dir "ccusage-perf.")]
        (try
          (let [context (prepare-context options install-root)]
            (if (:skip-reason context)
              (write-output! (:output options) (render-skipped-markdown (assoc context :reason (:skip-reason context))))
              (let [sections (run-benchmark-suites context)
                    diagnostic (when (:large-fixture-dir context)
                                 (runtime-diagnostic-section
                                  (merge context {:fixture-dir (:large-fixture-dir context)
                                                  :codex-fixture-dir (or (:large-codex-fixture-dir context) (:codex-fixture-dir context))
                                                  :commands ["claude" "codex"] :runs (:large-runs context) :warmup (:large-warmup context)
                                                  :title "Package runtime diagnostics"
                                                  :description "Compares the PR package wrapper, the installed native optional dependency binary, and the workspace release binary on the same large fixture. This identifies whether slow package results come from JavaScript wrapper overhead, the published native binary build, or the Rust core itself."})))
                    report (render-report (assoc context :runtime-diagnostic-sections (cond-> [] diagnostic (conj diagnostic)))
                                          sections (sizes-for context))]
                (write-output! (:output options) report))))
          (finally (fs/delete-tree install-root)))))))
