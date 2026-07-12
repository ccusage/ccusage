(defn command-args [command] (remove str/blank? (str/split command #" ")))
(defn benchmark-command [kind bin fixture-dir codex-fixture-dir command]
  (let [argv (vec (concat (if (= kind :package) ["node" bin] [bin])
                          (command-args command) ["--offline" "--json"]))
        env (benchmark-env fixture-dir codex-fixture-dir)]
    {:argv argv :env env :text (benchmark-command-text env argv)}))
(defn head-command [{:keys [head-runtime head-bin-entry head-native-bin-entry head-dir
                            fixture-dir codex-fixture-dir command]}]
  (cond
    (and (= head-runtime "package") head-bin-entry) (benchmark-command :package head-bin-entry fixture-dir codex-fixture-dir command)
    (and (= head-runtime "rust") head-native-bin-entry) (benchmark-command :rust head-native-bin-entry fixture-dir codex-fixture-dir command)
    (= head-runtime "rust") (benchmark-command :rust (rust-binary-entry head-dir) fixture-dir codex-fixture-dir command)
    :else (benchmark-command :package (package-bin-entry head-dir) fixture-dir codex-fixture-dir command)))

(defn parse-peak-rss [stderr]
  (if-let [match (re-find #"Maximum resident set size \(kbytes\):\s*(\d+)" stderr)]
    (* 1024 (parse-long (second match)))
    (if-let [match (re-find #"(?m)^\s*(\d+)\s+maximum resident set size$" stderr)]
      (parse-long (second match))
      (fail! "Could not parse peak RSS from time output" {:kind :malformed-time-output}))))
(defn timed-command [command]
  (case (platform-name)
    "linux" (into ["/usr/bin/time" "-v"] (:argv command))
    "darwin" (into ["/usr/bin/time" "-l"] (:argv command))
    nil))
(defn measure-memory [command {:keys [runs label]}]
  (when (pos? runs)
    (let [samples (keep (fn [_]
                          (if-let [argv (timed-command command)]
                            (let [result (run-process argv {:env (:env command)})]
                              (if (zero? (:exit result))
                                (try (parse-peak-rss (:err result))
                                     (catch Exception e (write-progress (str label " peak RSS skipped: " (ex-message e))) nil))
                                (do (write-progress (format "%s peak RSS skipped after exit %d: %s" label (:exit result) (:err result))) nil)))
                            (fail! (str "Peak RSS measurement is not supported on " (platform-name)) {:kind :unsupported-platform})))
                        (range runs))]
      (when (seq samples)
        {:peak-rss-bytes (nth (vec (sort samples)) (quot (count samples) 2)) :samples (count samples)}))))

(defn read-hyperfine-results [export-path expected]
  (let [data (try (json/parse-string (slurp export-path) true)
                  (catch Exception e (fail! (str "Malformed hyperfine output: " (ex-message e))
                                             {:kind :malformed-hyperfine :path (str export-path)})))
        results (:results data)]
    (when-not (= expected (count results))
      (fail! (format "hyperfine reported %d results, expected %d" (count results) expected)
             {:kind :malformed-hyperfine :expected expected :actual (count results)}))
    results))

(def hyperfine-common ["--shell" "none" "--style" "basic" "--output" "pipe" "--sort" "command"])
(defn run-hyperfine [commands names runs warmup label]
  (let [dir (temp-dir "ccusage-hyperfine.") export (path dir "hyperfine.json")
        argv (vec (concat ["hyperfine" "--shell" "none" "--warmup" (str warmup) "--runs" (str runs)
                           "--export-json" export "--style" "basic" "--output" "pipe" "--sort" "command"]
                          (mapcat #(vector "--command-name" %) names) (map :text commands)))]
    (try
      (run-checked argv {} (str "hyperfine failed for " label))
      (mapv measurement-from-hyperfine (read-hyperfine-results export (count commands)))
      (finally (fs/delete-tree dir)))))

(defn compare-command [command options]
  (let [label (str (:title options) " / " command)
        _ (write-progress (str label " started"))
        base-command (benchmark-command :package (:base-bin-entry options) (:fixture-dir options) (:codex-fixture-dir options) command)
        head-command* (head-command (assoc options :command command))
        [base head] (run-hyperfine [base-command head-command*] ["base" "PR"] (:runs options) (:warmup options) label)
        base-memory (measure-memory base-command {:runs (:memory-runs options) :label (str label " base")})
        head-memory (measure-memory head-command* {:runs (:memory-runs options) :label (str label " PR")})]
    (write-progress (format "%s done: base %s, PR %s" label (format-duration (:median base)) (format-duration (:median head))))
    {:base base :base-memory base-memory :command command :head head :head-memory head-memory}))

(defn compare-fixture [options]
  (write-progress (str (:title options) " started"))
  (let [section {:codex-fixture-dir (:codex-fixture-dir options)
                 :codex-fixture-stats (when (:codex-fixture-dir options) (summarize-directory (:codex-fixture-dir options)))
                 :description (:description options) :fixture-dir (:fixture-dir options)
                 :fixture-stats (summarize-directory (:fixture-dir options))
                 :memory-runs (:memory-runs options)
                 :results (mapv #(compare-command % options) (:commands options))
                 :runs (:runs options) :title (:title options) :warmup (:warmup options)}]
    (write-progress (str (:title options) " finished")) section))

(defn runtime-diagnostic-section [options]
  (let [workspace-bin (rust-binary-entry (:head-dir options))
        variants (cond-> []
                   (:head-bin-entry options) (conj {:label "Package wrapper" :kind :package :bin (:head-bin-entry options)})
                   (:head-native-bin-entry options) (conj {:label "Installed native binary" :kind :rust :bin (:head-native-bin-entry options)})
                   (optional-file-size workspace-bin) (conj {:label "Workspace release binary" :kind :rust :bin workspace-bin}))]
    (when (>= (count variants) 2)
      (write-progress (str (:title options) " started"))
      (let [results (mapcat
                     (fn [command]
                       (let [label (str (:title options) " / runtime diagnostics (" command ")")
                             _ (write-progress (str label " started"))
                             commands (mapv #(benchmark-command (:kind %) (:bin %) (:fixture-dir options) (:codex-fixture-dir options) command) variants)
                             measurements (run-hyperfine commands (mapv :label variants) (:runs options) (:warmup options) label)]
                         (write-progress (str label " done: " (str/join ", " (map #(str (:label %1) " " (format-duration (:median %2))) variants measurements))))
                         (mapv (fn [variant measurement] {:command command :label (:label variant) :measurement measurement}) variants measurements)))
                     (:commands options))
            section {:codex-fixture-dir (:codex-fixture-dir options)
                     :codex-fixture-stats (when (:codex-fixture-dir options) (summarize-directory (:codex-fixture-dir options)))
                     :description (:description options) :fixture-dir (:fixture-dir options)
                     :fixture-stats (summarize-directory (:fixture-dir options))
                     :results (vec results) :runs (:runs options) :title (:title options) :warmup (:warmup options)}]
        (write-progress (str (:title options) " finished")) section))))
