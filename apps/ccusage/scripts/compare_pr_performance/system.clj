(ns compare-pr-performance.system
  (:require [babashka.fs :as fs]
            [babashka.process :as process]
            [clojure.string :as str]
            [compare-pr-performance.data :as data]))

(defn run-process
  ([argv] (run-process argv {}))
  ([argv {:keys [dir env inherit?] :or {env {}}}]
   (let [opts (cond-> {:env (merge (into {} (System/getenv)) env)}
                dir (assoc :dir (str dir))
                inherit? (assoc :out :inherit :err :inherit)
                (not inherit?) (assoc :out :string :err :string))
         result @(process/process argv opts)]
     {:exit (:exit result) :out (or (:out result) "") :err (or (:err result) "")})))

(defn run-checked [argv options label]
  (let [result (run-process argv options)]
    (if (zero? (:exit result)) result
        (data/fail! (format "%s failed in %s: exit %d\n%s" label (or (:dir options) ".")
                       (:exit result) (str/trim (or (not-empty (:err result)) (:out result))))
               {:kind :child-command-failed :argv argv :dir (:dir options)
                :exit (:exit result) :stderr (:err result)}))))

(defn path [& parts] (str (apply fs/path parts)))
(defn expand [value] (str (fs/absolutize (fs/expand-home value))))
(defn write-output! [output markdown]
  (if output (spit (expand output) markdown) (print markdown)))

(defn temp-dir [prefix] (fs/create-temp-dir {:prefix prefix}))
(defn optional-file-size [file]
  (when (and file (fs/regular-file? file)) (fs/size file)))

(defn write-progress [message] (binding [*out* *err*] (println (str "[ccusage-perf] " message))))

(defn summarize-directory [directory]
  (let [files (filter fs/regular-file? (file-seq (fs/file directory)))]
    {:bytes (reduce + 0 (map fs/size files)) :files (count files)}))

(defn mtime-date
  [claude-dir position]
  (let [mtimes (->> (fs/glob claude-dir "**.jsonl")
                    (map #(.toMillis (fs/last-modified-time %)))
                    sort vec)]
    (when (empty? mtimes)
      (data/fail! (str "No Claude transcripts under " claude-dir) {:kind :empty-fixture :dir (str claude-dir)}))
    (.format (java.time.format.DateTimeFormatter/ofPattern "yyyyMMdd")
             (java.time.LocalDate/ofInstant (java.time.Instant/ofEpochMilli (nth mtimes (position (count mtimes))))
                                            java.time.ZoneOffset/UTC))))

(def mtime-positions
  {data/median-mtime #(quot % 2)
   data/recent-mtime #(quot (* 19 (dec %)) 20)})

(defn resolve-command [command claude-dir]
  (reduce (fn [command [placeholder position]]
            (if (str/includes? command placeholder)
              (str/replace command placeholder (mtime-date claude-dir position))
              command))
          command mtime-positions))

(def report-vars (sorted-map "COLUMNS" "200" "NO_COLOR" "1" "TZ" "UTC"))

(defn fixture-env
  [fixture-dir codex-fixture-dir grok-fixture-dir empty-home]
  {:clean? true
   :vars (into (cond-> (array-map "CLAUDE_CONFIG_DIR" fixture-dir)
                 codex-fixture-dir (assoc "CODEX_HOME" codex-fixture-dir)
                 grok-fixture-dir (assoc "GROK_HOME" grok-fixture-dir)
                 true (assoc "HOME" empty-home))
               (assoc report-vars "LOG_LEVEL" "0"))})

(defn shape-env
  [{:keys [env-var] :or {env-var "CLAUDE_CONFIG_DIR"}} dir empty-home]
  {:clean? true
   :vars (into (array-map env-var dir "HOME" empty-home) (assoc report-vars "LOG_LEVEL" "0"))})
