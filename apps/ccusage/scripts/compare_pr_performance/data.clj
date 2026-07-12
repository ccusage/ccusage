(require '[babashka.cli :as cli]
         '[babashka.fs :as fs]
         '[babashka.process :as process]
         '[cheshire.core :as json]
         '[clojure.string :as str])

(def cli-spec
  {:base-dir {:ref "DIR"}
   :base-package-url {:ref "URL"}
   :base-sha {:ref "SHA"}
   :head-dir {:ref "DIR"}
   :head-runtime {:default "package" :ref "package|rust"}
   :fixture-dir {:ref "DIR"}
   :codex-fixture-dir {:ref "DIR"}
   :output {:ref "FILE"}
   :runs {:coerce :long :default 7 :ref "N"}
   :warmup {:coerce :long :default 2 :ref "N"}
   :large-fixture-dir {:ref "DIR"}
   :large-codex-fixture-dir {:ref "DIR"}
   :large-runs {:coerce :long :default 1 :ref "N"}
   :large-warmup {:coerce :long :default 0 :ref "N"}
   :memory-runs {:coerce :long :default 1 :ref "N"}
   :head-package-url {:ref "URL"}
   :package-runner-timeout-ms {:coerce :long :default 120000 :ref "MS"}
   :help {:alias :h :coerce :boolean}})

(defn fail! [message data]
  (throw (ex-info message data)))

(defn parse-head-runtime [value]
  (if (#{"package" "rust"} value)
    value
    (fail! (format "Invalid head runtime: %s. Use package or rust." value)
           {:kind :invalid-runtime :value value})))

(defn assert-sample-options [runs warmup label]
  (when (< runs 1)
    (fail! (format "--%sruns must be a positive integer" label) {:kind :invalid-cli}))
  (when (< warmup 0)
    (fail! (format "--%swarmup must be a non-negative integer" label) {:kind :invalid-cli})))

(defn parse-cli [argv]
  (let [options (cli/parse-opts argv {:spec cli-spec :restrict true})]
    (when-not (:help options)
      (doseq [required [:head-dir :fixture-dir :codex-fixture-dir]]
        (when-not (get options required)
          (fail! (str "--" (str/replace (name required) "_" "-") " is required")
                 {:kind :invalid-cli :option required})))
      (assert-sample-options (:runs options) (:warmup options) "")
      (assert-sample-options (:large-runs options) (:large-warmup options) "large-")
      (when (< (:memory-runs options) 0)
        (fail! "--memory-runs must be a non-negative integer" {:kind :invalid-cli}))
      (when-not (or (:base-dir options) (:base-package-url options))
        (fail! "Either --base-dir or --base-package-url is required" {:kind :invalid-cli}))
      (parse-head-runtime (:head-runtime options)))
    options))

(defn help-text []
  (str "Usage: compare-pr-performance.bb [options]\n\n"
       (cli/format-opts {:spec cli-spec})))

(defn format-sha [sha] (subs sha 0 (min 12 (count sha))))

(defn package-url-sha [url]
  (if-let [[_ sha] (re-find #"@([0-9a-fA-F]{7,40})(?:$|[/?#])" url)]
    (format-sha sha)
    url))

(defn format-duration [milliseconds]
  (if (>= milliseconds 1000)
    (format "%.3fs" (/ (double milliseconds) 1000))
    (format "%.1fms" (double milliseconds))))

(defn format-size [bytes] (format "%.2f KiB" (/ (double bytes) 1024)))
(defn format-optional-size [bytes] (if (nil? bytes) "-" (format-size bytes)))
(defn format-size-delta [base head]
  (if (or (nil? base) (nil? head)) "-"
      (let [delta (- head base)] (str (when (>= delta 0) "+") (format-size delta)))))
(defn format-size-ratio [base head]
  (if (or (nil? base) (nil? head) (zero? head)) "-"
      (format "%.2fx" (/ (double base) head))))
(defn format-data-size [bytes]
  (if (>= bytes (* 1024 1024 1024))
    (format "%.2f GiB" (/ (double bytes) 1024 1024 1024))
    (format "%.2f MiB" (/ (double bytes) 1024 1024))))
(defn format-memory-size [bytes]
  (cond
    (>= bytes (* 1024 1024 1024)) (format "%.2f GiB" (/ (double bytes) 1024 1024 1024))
    (>= bytes (* 1024 1024)) (format "%.2f MiB" (/ (double bytes) 1024 1024))
    :else (format "%.2f KiB" (/ (double bytes) 1024))))
(defn format-optional-memory [m] (if m (format-memory-size (:peak-rss-bytes m)) "-"))
(defn format-memory-ratio [base head]
  (if (or (nil? base) (nil? head) (zero? (:peak-rss-bytes base))) "-"
      (format "%.2fx" (/ (double (:peak-rss-bytes head)) (:peak-rss-bytes base)))))
(defn format-throughput [bytes milliseconds]
  (let [mibps (/ (double bytes) 1024 1024 (/ milliseconds 1000.0))]
    (if (>= mibps 1024) (format "%.2f GiB/s" (/ mibps 1024)) (format "%.2f MiB/s" mibps))))

(defn measurement-from-milliseconds [times]
  (when (empty? times) (fail! "Cannot summarize zero measurements" {:kind :empty-measurement}))
  (let [sorted (vec (sort times))]
    {:max (peek sorted) :median (nth sorted (quot (count sorted) 2))
     :min (first sorted) :samples (count sorted)}))

(defn measurement-from-hyperfine [result]
  {:max (* 1000 (:max result)) :median (* 1000 (:median result))
   :min (* 1000 (:min result)) :samples (count (:times result))})

(def safe-arg-pattern #"^[A-Za-z0-9_@%+=:,./-]+$")
(defn command-text-arg [value]
  (let [text (str value)]
    (if (re-matches safe-arg-pattern text) text (json/generate-string text))))
(defn benchmark-command-text [env argv]
  (->> (concat ["env"] (map (fn [[k v]] (str k "=" v)) env) argv)
       (map command-text-arg)
       (str/join " ")))
