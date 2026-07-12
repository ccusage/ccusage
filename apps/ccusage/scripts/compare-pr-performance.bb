#!/usr/bin/env nix
#! nix shell --inputs-from ../../.. nixpkgs#babashka-unwrapped nixpkgs#bun nixpkgs#curl nixpkgs#git nixpkgs#hyperfine nixpkgs#nodejs nixpkgs#pnpm --command bb

(require '[babashka.fs :as fs])

(def script-dir (fs/parent (fs/file *file*)))

(doseq [source ["data.clj" "system.clj" "benchmark.clj" "report.clj" "main.clj"]]
  (load-file (str (fs/path script-dir "compare_pr_performance" source))))

(when (= *file* (System/getProperty "babashka.file"))
  (try
    (apply -main *command-line-args*)
    (catch Exception error
      (binding [*out* *err*]
        (println (ex-message error)))
      (System/exit 1))))
