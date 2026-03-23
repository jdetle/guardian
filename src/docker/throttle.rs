use std::process::Command;

pub fn pause_container(name: &str) -> Result<(), String> {
    let output = Command::new("docker")
        .args(["pause", name])
        .output()
        .map_err(|e| format!("exec docker pause: {e}"))?;

    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).to_string())
    }
}

pub fn unpause_container(name: &str) -> Result<(), String> {
    let output = Command::new("docker")
        .args(["unpause", name])
        .output()
        .map_err(|e| format!("exec docker unpause: {e}"))?;

    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).to_string())
    }
}

pub fn update_cpu_limit(name: &str, cpus: f64) -> Result<(), String> {
    let output = Command::new("docker")
        .args(["update", "--cpus", &cpus.to_string(), name])
        .output()
        .map_err(|e| format!("exec docker update: {e}"))?;

    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).to_string())
    }
}

pub fn remove_cpu_limit(name: &str) -> Result<(), String> {
    update_cpu_limit(name, 0.0)
}

pub fn throttle_non_essential(
    all_containers: &[String],
    essential: &[String],
    cpu_limit: f64,
) {
    for name in all_containers {
        if is_essential(name, essential) {
            continue;
        }
        if let Err(e) = update_cpu_limit(name, cpu_limit) {
            eprintln!("[guardian] throttle {name}: {e}");
        }
    }
}

fn is_essential(name: &str, essential: &[String]) -> bool {
    essential.iter().any(|e| {
        name == e.as_str()
            || name.starts_with(&format!("{e}-"))
            || name.starts_with(&format!("{e}_"))
    })
}

pub fn unthrottle_all(all_containers: &[String]) {
    for name in all_containers {
        if let Err(e) = remove_cpu_limit(name) {
            eprintln!("[guardian] unthrottle {name}: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn essential(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn exact_match_is_essential() {
        assert!(is_essential("postgres", &essential(&["postgres"])));
    }

    #[test]
    fn dash_suffix_is_essential() {
        assert!(is_essential("postgres-1", &essential(&["postgres"])));
    }

    #[test]
    fn underscore_suffix_is_essential() {
        assert!(is_essential("postgres_replica", &essential(&["postgres"])));
    }

    #[test]
    fn unrelated_name_is_not_essential() {
        assert!(!is_essential("redis", &essential(&["postgres"])));
    }

    #[test]
    fn partial_overlap_without_separator_is_not_essential() {
        assert!(!is_essential("postgresqldb", &essential(&["postgres"])));
    }

    #[test]
    fn empty_essential_list() {
        assert!(!is_essential("anything", &essential(&[])));
    }

    #[test]
    fn multiple_essential_entries() {
        let ess = essential(&["postgres", "redis"]);
        assert!(is_essential("postgres", &ess));
        assert!(is_essential("redis-cluster", &ess));
        assert!(!is_essential("mysql", &ess));
    }
}
