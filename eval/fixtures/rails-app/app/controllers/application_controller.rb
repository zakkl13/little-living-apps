class ApplicationController < ActionController::Base
  # NOTE (eval fixture): the default `allow_browser versions: :modern` would 406 plain HTTP probes
  # and curl-based worker self-validation, so it is intentionally omitted here.
end
