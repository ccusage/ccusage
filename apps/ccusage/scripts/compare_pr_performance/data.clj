(ns compare-pr-performance.data
  (:require [babashka.cli :as cli]
            [cheshire.core :as json]
            [clojure.string :as str]))

(def default-commands ["claude daily" "claude session" "codex daily" "daily"])

(def shape-fixtures
  [{:fixture :many-files :option :many-files-fixture-dir :label "many small files"
    :desc "Claude logs spread as many small files over many project directories, so discovery dominates"}
   {:fixture :proxy :option :proxy-fixture-dir :label "requestless proxy"
    :desc "Claude logs written through a gateway that drops `requestId`, so responses are deduplicated along `uuid`/`parentUuid` chains"}
   {:fixture :small :option :small-fixture-dir :label "tiny"
    :desc "a few MiB of Claude logs, so fixed startup cost such as building the pricing map dominates"}
   {:fixture :opencode :option :opencode-fixture-dir :label "large OpenCode database" :env-var "OPENCODE_DATA_DIR"
    :desc "a WAL-mode `opencode.db` whose `session_message` payloads, up to 48 KiB, hold long content before their token counts, so reading payloads dominates"}])

(def median-mtime "{median-mtime}")
(def recent-mtime "{recent-mtime}")

(def shape-commands
  [{:fixture :large :command (str "claude daily --since " median-mtime)}
   {:fixture :large :command (str "claude daily --since " recent-mtime)}
   {:fixture :large :command "codex session"}
   {:fixture :many-files :command "claude daily"}
   {:fixture :proxy :command "claude daily"}
   {:fixture :small :command "daily"}
   {:fixture :opencode :command "opencode daily"}])

(def cli-spec
  {:base-bin {:ref "FILE" :desc "Base ccusage binary, built from the PR's base commit"}
   :base-sha {:ref "SHA"}
   :head-bin {:ref "FILE" :desc "PR ccusage binary, built the same way as the base"}
   :head-sha {:ref "SHA"}
   :fixture-dir {:ref "DIR" :desc "Claude fixture (CLAUDE_CONFIG_DIR)"}
   :codex-fixture-dir {:ref "DIR" :desc "Codex fixture (CODEX_HOME)"}
   :grok-fixture-dir {:ref "DIR" :desc "Grok fixture (GROK_HOME); optional"}
   :many-files-fixture-dir {:ref "DIR" :desc "Claude fixture generated with --claude-shape many-small-files; optional"}
   :proxy-fixture-dir {:ref "DIR" :desc "Claude fixture generated with --claude-shape requestless-proxy; optional"}
   :small-fixture-dir {:ref "DIR" :desc "Claude fixture of a few MiB; optional"}
   :opencode-fixture-dir {:ref "DIR" :desc "OpenCode fixture (OPENCODE_DATA_DIR) generated with --opencode-output-dir; optional"}
   :output {:ref "FILE"}
   :runs {:coerce :long :default 11 :ref "N" :desc "Interleaved measured rounds; at least 3"}
   :warmup {:coerce :long :default 2 :ref "N" :desc "Interleaved warmup rounds"}
   :threshold {:coerce :double :default 0.1 :ref "RATIO" :desc "CPU-time and RSS change that counts as a regression"}
   :help {:alias :h :coerce :boolean}})

(defn fail! [message data]
  (throw (ex-info message data)))

(defn assert-sample-options [{:keys [runs warmup threshold]}]
  (when (< runs 3)
    (fail! "--runs must be at least 3" {:kind :invalid-cli}))
  (when (< warmup 0)
    (fail! "--warmup must be a non-negative integer" {:kind :invalid-cli}))
  (when-not (< 0 threshold 1)
    (fail! "--threshold must be between 0 and 1" {:kind :invalid-cli})))

(defn parse-cli [argv]
  (let [options (cli/parse-opts argv {:spec cli-spec :restrict true})]
    (when-not (:help options)
      (doseq [required [:base-bin :head-bin :fixture-dir :codex-fixture-dir]]
        (when-not (get options required)
          (fail! (str "--" (name required) " is required")
                 {:kind :invalid-cli :option required})))
      (assert-sample-options options))
    options))

(defn help-text []
  (str "Usage: compare-pr-performance.bb [options]\n\n"
       (cli/format-opts {:spec cli-spec})))

(defn format-sha [sha] (subs sha 0 (min 12 (count sha))))

(defn format-duration [milliseconds]
  (if (>= milliseconds 1000)
    (format "%.2fs" (/ (double milliseconds) 1000))
    (format "%.0fms" (double milliseconds))))

(defn format-size [bytes] (format "%.2f KiB" (/ (double bytes) 1024)))
(defn format-optional-size [bytes] (if (nil? bytes) "-" (format-size bytes)))
(defn format-size-delta [base head]
  (if (or (nil? base) (nil? head)) "-"
      (let [delta (- head base)] (str (when (>= delta 0) "+") (format-size delta)))))
(defn format-data-size [bytes]
  (if (>= bytes (* 1024 1024 1024))
    (format "%.2f GiB" (/ (double bytes) 1024 1024 1024))
    (format "%.2f MiB" (/ (double bytes) 1024 1024))))
(defn format-memory-size [bytes]
  (cond
    (>= bytes (* 1024 1024 1024)) (format "%.2f GiB" (/ (double bytes) 1024 1024 1024))
    (>= bytes (* 1024 1024)) (format "%.0f MiB" (/ (double bytes) 1024 1024))
    :else (format "%.0f KiB" (/ (double bytes) 1024))))
(defn format-ratio [ratio] (format "%.2fx" (double ratio)))

(defn median [values]
  (when (empty? values) (fail! "Cannot summarize zero measurements" {:kind :empty-measurement}))
  (let [sorted (vec (sort values)) n (count sorted)]
    (if (odd? n)
      (nth sorted (quot n 2))
      (/ (+ (nth sorted (dec (quot n 2))) (nth sorted (quot n 2))) 2.0))))

(defn summarize [values]
  {:median (median values) :min (apply min values) :max (apply max values) :samples (count values)})

(defn classify-ratios
  [ratios threshold]
  (let [n (count ratios)
        agreeing (- n (quot n 5))
        middle (median ratios)
        above (count (filter #(> % 1.0) ratios))
        below (count (filter #(< % 1.0) ratios))]
    (cond
      (and (> middle (+ 1.0 threshold)) (>= above agreeing)) :regression
      (and (< middle (- 1.0 threshold)) (>= below agreeing)) :improvement
      :else :unchanged)))

(def safe-arg-pattern #"^[A-Za-z0-9_@%+=:,./-]+$")
(defn command-text-arg [value]
  (let [text (str value)]
    (if (re-matches safe-arg-pattern text) text (json/generate-string text))))
(defn benchmark-command-text
  [{:keys [vars clean?]} argv]
  (->> (concat ["env"] (when clean? ["-i"]) (map (fn [[k v]] (str k "=" v)) vars) argv)
       (map command-text-arg)
       (str/join " ")))
