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

(defn normalize-platform-name [os-name]
  (cond
    (= os-name "Mac OS X") "darwin"
    (= os-name "Linux") "linux"
    (str/starts-with? os-name "Windows") "win32"
    :else (str/lower-case os-name)))
(defn platform-name [] (normalize-platform-name (System/getProperty "os.name")))
(defn arch-name [] ({"aarch64" "arm64" "x86_64" "x64" "amd64" "x64"}
                     (System/getProperty "os.arch") (System/getProperty "os.arch")))
(defn path [& parts] (str (apply fs/path parts)))
(defn expand [value] (str (fs/absolutize (fs/expand-home value))))
(defn temp-dir [prefix] (fs/create-temp-dir {:prefix prefix}))
(defn optional-file-size [file]
  (when (and file (fs/regular-file? file)) (fs/size file)))

(defn package-bin-entry [repo-dir]
  (let [package-dir (path repo-dir "apps" "ccusage")
        package-json (json/parse-string (slurp (path package-dir "package.json")) true)
        bin-path (or (get-in package-json [:publishConfig :bin :ccusage])
                     (get-in package-json [:bin :ccusage]))]
    (when-not bin-path
      (fail! (str "ccusage bin is missing in " package-dir "/package.json")
             {:kind :missing-package-binary :dir package-dir}))
    (path package-dir bin-path)))
(defn rust-binary-entry [repo-dir]
  (path repo-dir "rust" "target" "release" (if (= "win32" (platform-name)) "ccusage.exe" "ccusage")))
(defn package-bin-shim [install-dir]
  (path install-dir "node_modules" ".bin" (if (= "win32" (platform-name)) "ccusage.cmd" "ccusage")))
(defn installed-package-bin-entry [install-dir]
  (let [package-dir (path install-dir "node_modules" "ccusage")
        json-path (path package-dir "package.json")
        bin-path (when (fs/exists? json-path)
                   (get-in (json/parse-string (slurp json-path) true) [:bin :ccusage]))]
    (if bin-path (path package-dir bin-path) (package-bin-shim install-dir))))
(defn native-package-directory-name [platform arch]
  (when (and (#{"darwin" "linux" "win32"} platform) (#{"arm64" "x64"} arch))
    (str "ccusage-" platform "-" arch)))
(defn installed-native-package-bin-entry [install-dir]
  (when-let [dirname (native-package-directory-name (platform-name) (arch-name))]
    (let [bin (path install-dir "node_modules" "@ccusage" dirname "bin"
                    (if (= "win32" (platform-name)) "ccusage.exe" "ccusage"))]
      (when (optional-file-size bin)
        (when-not (= "win32" (platform-name)) (run-process ["chmod" "+x" bin]))
        bin))))

(defn write-progress [message] (binding [*out* *err*] (println (str "[ccusage-perf] " message))))
(defn git-sha [directory]
  (str/trim (:out (run-checked ["git" "rev-parse" "HEAD"] {:dir directory} "git rev-parse HEAD"))))
(defn package-url-ready? [url]
  (zero? (:exit (run-process ["curl" "--fail" "--head" "--location" "--silent" "--output" "/dev/null" url]))))
(defn wait-for-package-url [url timeout-ms]
  (let [deadline (+ (System/currentTimeMillis) timeout-ms)]
    (loop []
      (cond (>= (System/currentTimeMillis) deadline) false
            (package-url-ready? url) true
            :else (do (Thread/sleep 5000) (recur))))))

(defn install-package-url [install-dir label url timeout-ms]
  (write-progress (str label " package install waiting for package URL"))
  (if-not (wait-for-package-url url timeout-ms)
    (do (write-progress (str label " package install skipped because package URL was not ready")) nil)
    (do
      (fs/create-dirs install-dir)
      (spit (path install-dir "package.json")
            (json/generate-string {:private true :dependencies {:ccusage url}} {:pretty true}))
      (write-progress (str label " package install started: " url))
      (let [started (System/currentTimeMillis)
            result (run-process ["bun" "install" "--no-progress"] {:dir install-dir})
            elapsed (- (System/currentTimeMillis) started)]
        (when-not (zero? (:exit result))
          (fail! (str label " package install failed: "
                      (str/trim (or (not-empty (:err result)) (:out result))))
                 {:kind :package-install-failed :label label :url url :exit (:exit result)}))
        (write-progress (str label " package install finished: " (format-duration elapsed)))
        {:acquisition elapsed :bin-entry (installed-package-bin-entry install-dir)}))))

(defn base-source-decision [{:keys [package-install base-dir base-package-url]}]
  (cond package-install :installed base-dir :local base-package-url :skip))

(defn packed-tarball-path [destination filename]
  (if (fs/absolute? filename) filename (path destination filename)))

(defn packed-tarball-size [repo-dir]
  (let [package-dir (path repo-dir "apps" "ccusage")
        package-json-path (path package-dir "package.json")
        original (slurp package-json-path)
        destination (temp-dir "ccusage-pack.")]
    (try
      (let [result (run-process ["pnpm" "pack" "--json" "--pack-destination" (str destination)] {:dir package-dir})]
        (when-not (zero? (:exit result))
          (fail! (str "pnpm pack failed: " (str/trim (or (not-empty (:err result)) (:out result))))
                 {:kind :package-metadata-failed :exit (:exit result)}))
        (let [lines (str/split-lines (:out result))
              start (last (keep-indexed #(when (str/starts-with? %2 "{") %1) lines))
              pack-result (when start (json/parse-string (str/join "\n" (drop start lines)) true))
              filename (:filename pack-result)]
          (when-not filename
            (fail! "pnpm pack did not report a tarball filename" {:kind :malformed-pack-output}))
          (optional-file-size (packed-tarball-path destination filename))))
      (finally (spit package-json-path original) (fs/delete-tree destination)))))

(defn remote-tarball-size [url]
  (let [output (fs/create-temp-file {:prefix "ccusage-package."})]
    (try
      (run-checked ["curl" "--fail" "--location" "--silent" "--output" (str output) url]
                   {} (str "Failed to fetch " url))
      (optional-file-size output)
      (finally (fs/delete-if-exists output)))))

(defn summarize-directory [directory]
  (let [files (filter fs/regular-file? (file-seq (fs/file directory)))]
    {:bytes (reduce + 0 (map fs/size files)) :files (count files)}))
(defn benchmark-env [fixture-dir codex-fixture-dir]
  (cond-> (array-map "CLAUDE_CONFIG_DIR" fixture-dir "COLUMNS" "200" "LOG_LEVEL" "0"
                     "NO_COLOR" "1" "TZ" "UTC")
    codex-fixture-dir (assoc "CODEX_HOME" codex-fixture-dir)))
