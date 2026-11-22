#!/usr/bin/env nix
#! nix shell --inputs-from ../../.. nixpkgs#babashka-unwrapped nixpkgs#bash nixpkgs#time nixpkgs#hyperfine --command bb

(ns compare-pr-performance
  (:require [babashka.classpath :as cp]
            [babashka.fs :as fs]))

(cp/add-classpath (str (fs/parent (fs/absolutize *file*))))

(require '[compare-pr-performance.main :as main])

(when (= *file* (System/getProperty "babashka.file"))
  (try
    (apply main/-main *command-line-args*)
    (catch Exception error
      (binding [*out* *err*]
        (println (ex-message error)))
      (System/exit 1))))
