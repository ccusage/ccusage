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
        (fail! (format "%s failed in %s: exit %d\n%s" label (or (:dir options) ".")
                       (:exit result) (str/trim (or (not-empty (:err result)) (:out result))))
               {:kind :child-command-failed :argv argv :dir (:dir options)
                :exit (:exit result) :stderr (:err result)}))))

(defn path [& parts] (str (apply fs/path parts)))
(defn expand [value] (str (fs/absolutize (fs/expand-home value))))
(defn temp-dir [prefix] (fs/create-temp-dir {:prefix prefix}))
(defn optional-file-size [file]
  (when (and file (fs/regular-file? file)) (fs/size file)))

(defn write-progress [message] (binding [*out* *err*] (println (str "[ccusage-perf] " message))))

(defn summarize-directory [directory]
  (let [files (filter fs/regular-file? (file-seq (fs/file directory)))]
    {:bytes (reduce + 0 (map fs/size files)) :files (count files)}))

(defn fixture-env
  "Points every agent at the fixture. HOME moves to an empty directory so the
  unified report cannot pick up agents that happen to have logs on this machine."
  [fixture-dir codex-fixture-dir empty-home]
  (array-map "CLAUDE_CONFIG_DIR" fixture-dir "CODEX_HOME" codex-fixture-dir "HOME" empty-home
             "XDG_CONFIG_HOME" (path empty-home ".config") "XDG_DATA_HOME" (path empty-home ".local" "share")
             "XDG_STATE_HOME" (path empty-home ".local" "state") "XDG_CACHE_HOME" (path empty-home ".cache")
             "COLUMNS" "200" "LOG_LEVEL" "0" "NO_COLOR" "1" "TZ" "UTC"))
