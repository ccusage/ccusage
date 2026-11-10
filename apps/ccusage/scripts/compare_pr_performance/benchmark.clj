(defn command-args [command] (remove str/blank? (str/split command #" ")))
(defn command-argv [bin command] (vec (concat [bin] (command-args command) ["--offline" "--json"])))

(def metric-keys
  {:wall :time_wall_clock :cpu :time_cpu :rss :memory_peak_resident :instructions :instructions})

(defn run-sample
  "Normalizes one hyperfine run: times in milliseconds, RSS in bytes.
  Instructions stay nil where the platform exposes no hardware counters."
  [measurement]
  (let [value (fn [k] (get-in measurement [(metric-keys k) :value]))
        seconds (fn [k]
                  (let [unit (get-in measurement [(metric-keys k) :unit])]
                    (when-not (= "second" unit)
                      (fail! (str "Unexpected hyperfine time unit: " (pr-str unit)) {:kind :malformed-hyperfine :metric k}))
                    (* 1000.0 (value k))))]
    {:wall (seconds :wall) :cpu (seconds :cpu) :rss (value :rss) :instructions (value :instructions)}))

(defn read-hyperfine-runs
  "Returns one sample per requested command name from a single-run hyperfine export."
  [export-path names]
  (let [data (try (json/parse-string (slurp export-path) true)
                  (catch Exception e (fail! (str "Malformed hyperfine output: " (ex-message e))
                                            {:kind :malformed-hyperfine :path (str export-path)})))
        _ (when-not (= 2 (:schema_version data))
            (fail! (str "Unsupported hyperfine JSON schema version: " (pr-str (:schema_version data)))
                   {:kind :malformed-hyperfine :path (str export-path)}))
        by-name (group-by :name (:results data))]
    (into {} (map (fn [name]
                    (let [matches (get by-name name)]
                      (when-not (= 1 (count matches))
                        (fail! (format "hyperfine reported %d results named %s, expected 1" (count matches) name)
                               {:kind :malformed-hyperfine :name name :actual (count matches)}))
                      [name (run-sample (first (:measurements (first matches))))])))
          names)))

(defn rotate [items n]
  (let [k (mod n (count items))] (vec (concat (drop k items) (take k items)))))

(defn hyperfine-round [variants command env label]
  (let [dir (temp-dir "ccusage-hyperfine.") export (path dir "hyperfine.json")
        argv (vec (concat ["hyperfine" "--shell" "none" "--runs" "1" "--export-json" export
                           "--style" "none" "--output" "pipe"]
                          (mapcat #(vector "--command-name" (:label %)) variants)
                          (map #(benchmark-command-text env (command-argv (:bin %) command)) variants)))]
    (try
      (run-checked argv {} (str "hyperfine failed for " label))
      (read-hyperfine-runs export (mapv :label variants))
      (finally (fs/delete-tree dir)))))

(defn measure-command
  "Runs every variant once per round, rotating the order each round, so slow
  drift on a shared runner hits all variants alike and per-round ratios pair up."
  [variants command env {:keys [runs warmup title]}]
  (let [label (str title " / " command)]
    (write-progress (str label " started"))
    (doseq [round (range warmup)]
      (hyperfine-round (rotate variants round) command env label))
    (let [rounds (mapv #(hyperfine-round (rotate variants %) command env label) (range runs))]
      (write-progress (str label " done"))
      {:command command :rounds rounds})))

(defn variant-summary [rounds label]
  (let [samples (map #(get % label) rounds)
        instructions (map :instructions samples)]
    {:wall (summarize (map :wall samples)) :cpu (summarize (map :cpu samples))
     :rss (summarize (map :rss samples))
     :instructions (when (every? some? instructions) (summarize instructions))}))

(defn paired-ratios [rounds metric reference variant]
  (mapv #(/ (double (get-in % [variant metric])) (get-in % [reference metric])) rounds))

(defn compare-variants
  "Compares variant against reference on CPU time and peak RSS; wall time is
  reported but not judged because it swings with runner load."
  [rounds reference variant threshold]
  (let [cpu (paired-ratios rounds :cpu reference variant)
        rss (paired-ratios rounds :rss reference variant)
        instructions (when (every? #(and (get-in % [reference :instructions]) (get-in % [variant :instructions])) rounds)
                       (paired-ratios rounds :instructions reference variant))]
    {:cpu (assoc (summarize cpu) :verdict (classify-ratios cpu threshold))
     :rss (assoc (summarize rss) :verdict (classify-ratios rss threshold))
     :instructions (when instructions (summarize instructions))}))
