#!/usr/bin/env nix
#! nix shell --inputs-from ../../.. nixpkgs#babashka-unwrapped nixpkgs#git nixpkgs#jq nixpkgs#diffutils --command bb

(require '[babashka.fs :as fs])

(def script-dir (fs/parent (fs/file *file*)))

(doseq [source ["compare_pr_performance/data.clj" "compare_pr_performance/system.clj"
                "compare_pr_performance/benchmark.clj" "compare_pr_performance/report.clj"
                "compare_pr_performance/main.clj" "compare_pr_performance/history.clj"
                "diff_refs/main.clj"]]
  (load-file (str (fs/path script-dir source))))

(when (= *file* (System/getProperty "babashka.file"))
  (System/exit
   (try
     (apply diff-main *command-line-args*)
     (catch Exception error
       (binding [*out* *err*]
         (println (ex-message error)))
       1))))
