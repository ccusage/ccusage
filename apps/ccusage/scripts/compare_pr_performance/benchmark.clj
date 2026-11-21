(ns compare-pr-performance.benchmark
  (:require [babashka.fs :as fs]
            [cheshire.core :as json]
            [clojure.string :as str]
            [compare-pr-performance.data :as data]
            [compare-pr-performance.system :as system]))

(defn command-args [command] (remove str/blank? (str/split command #" ")))
(defn command-argv [bin command] (vec (concat [bin] (command-args command) ["--offline" "--json"])))

;; NOTE(2026-11-21, #2449): Under hyperfine 2.0, ccusage's parallel OpenCode reads spent 1.6x the wall time and 1.6-3x the kernel time they do under /usr/bin/time, with or without hardware counters. Time each run with the shell's own `time`, and use hyperfine only for instruction counts.
;; NOTE(2026-11-21, #2449): bash 5.3 rounds a three-digit `time` fraction without carrying, so 0.9998 s prints as `0.:00`. Six digits are the exact microseconds and never round.
(def timing-script
  "TIMEFORMAT='%6R %6U %6S'; times=$1 errors=$2; shift 2; { time \"$@\" >/dev/null 2>\"$errors\"; } 2>\"$times\"")

(defn parse-timing [times rss-kib]
  (let [[wall user system] (map #(Double/parseDouble %) (str/split (str/trim times) #"\s+"))]
    {:wall (* 1000.0 wall) :cpu (* 1000.0 (+ user system)) :rss (* 1024 (Long/parseLong (str/trim rss-kib)))}))

(defn timed-run [argv label]
  (let [dir (system/temp-dir "ccusage-timed.")
        [times errors rss] (map #(system/path dir %) ["times" "stderr" "rss"])]
    (try
      (let [result (system/run-process (into ["bash" "-c" timing-script "timed-run" times errors "time" "-f" "%M" "-o" rss] argv)
                                       {:env {"LC_ALL" "C"}})]
        (when-not (zero? (:exit result))
          (data/fail! (format "%s failed: exit %d\n%s" label (:exit result)
                              (str/trim (str (when (fs/exists? errors) (slurp errors)) (:err result))))
                      {:kind :child-command-failed :argv argv :exit (:exit result)}))
        (parse-timing (slurp times) (last (str/split-lines (slurp rss)))))
      (finally (fs/delete-tree dir)))))

(defn read-hyperfine-instructions
  [export-path names]
  (let [data (try (json/parse-string (slurp export-path) true)
                  (catch Exception e (data/fail! (str "Malformed hyperfine output: " (ex-message e))
                                            {:kind :malformed-hyperfine :path (str export-path)})))
        _ (when-not (= 2 (:schema_version data))
            (data/fail! (str "Unsupported hyperfine JSON schema version: " (pr-str (:schema_version data)))
                   {:kind :malformed-hyperfine :path (str export-path)}))
        by-name (group-by :name (:results data))]
    (into {} (map (fn [name]
                    (let [matches (get by-name name)]
                      (when-not (= 1 (count matches))
                        (data/fail! (format "hyperfine reported %d results named %s, expected 1" (count matches) name)
                               {:kind :malformed-hyperfine :name name :actual (count matches)}))
                      [name (get-in (first matches) [:measurements 0 :instructions :value])])))
          names)))

(defn rotate [items n]
  (let [k (mod n (count items))] (vec (concat (drop k items) (take k items)))))

(defn timing-round [variants command env label]
  (into {} (map (fn [{variant :label bin :bin}]
                  [variant (timed-run (data/benchmark-argv env (command-argv bin command)) (str label " / " variant))]))
        variants))

(defn instruction-counts [variants command env label]
  (let [dir (system/temp-dir "ccusage-hyperfine.") export (system/path dir "hyperfine.json")
        argv (vec (concat ["hyperfine" "--shell" "none" "--runs" "1" "--metrics" "instructions" "--export-json" export
                           "--style" "none" "--output" "pipe"]
                          (mapcat #(vector "--command-name" (:label %)) variants)
                          (map #(data/benchmark-command-text env (command-argv (:bin %) command)) variants)))]
    (try
      (system/run-checked argv {} (str "hyperfine failed for " label))
      (read-hyperfine-instructions export (mapv :label variants))
      (finally (fs/delete-tree dir)))))

(defn measure-command
  [variants command env {:keys [runs warmup title]}]
  (let [label (str title " / " command)]
    (system/write-progress (str label " started"))
    (doseq [round (range warmup)]
      (timing-round (rotate variants round) command env label))
    (let [rounds (mapv #(timing-round (rotate variants %) command env label) (range runs))
          instructions (instruction-counts variants command env label)
          rounds (mapv (fn [round] (reduce-kv #(assoc-in %1 [%2 :instructions] %3) round instructions)) rounds)]
      (system/write-progress (str label " done"))
      {:command command :rounds rounds})))

(defn variant-summary [rounds label]
  (let [samples (map #(get % label) rounds)
        instructions (map :instructions samples)]
    {:wall (data/summarize (map :wall samples)) :cpu (data/summarize (map :cpu samples))
     :rss (data/summarize (map :rss samples))
     :instructions (when (every? some? instructions) (data/summarize instructions))}))

(defn paired-ratios [rounds metric reference variant]
  (mapv #(/ (double (get-in % [variant metric])) (get-in % [reference metric])) rounds))

(defn compare-variants
  [rounds reference variant {:keys [threshold wall-threshold]}]
  (let [wall (paired-ratios rounds :wall reference variant)
        cpu (paired-ratios rounds :cpu reference variant)
        rss (paired-ratios rounds :rss reference variant)
        instructions (when (every? #(and (get-in % [reference :instructions]) (get-in % [variant :instructions])) rounds)
                       (paired-ratios rounds :instructions reference variant))]
    {:wall (assoc (data/summarize wall) :verdict (data/classify-ratios wall wall-threshold))
     :cpu (assoc (data/summarize cpu) :verdict (data/classify-ratios cpu threshold))
     :rss (assoc (data/summarize rss) :verdict (data/classify-ratios rss threshold))
     :instructions (when instructions (data/summarize instructions))}))
