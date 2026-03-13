include "root" {
  path = find_in_parent_folders("root.hcl")
}

terraform {
  source = "../../skyr-k8s"
}

inputs = {
  namespace = "skyr-dev"
}
