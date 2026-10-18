grep -rlw compute_tax --include=*.py --include=*.md . | xargs perl -pi -e 's/\bcompute_tax\b/calculate_tax/g'
