# Lilapp — the tiny app the team builds and maintains (Rails 8). Mirrors the Node fixture's surface
# so the same behavior scenarios apply: a plain root and a /greet endpoint.
class HomeController < ApplicationController
  def index
    render plain: "Lilapp is running\n"
  end

  def greet
    name = params[:name] || "world"
    render plain: "Hello, #{name.strip}!\n"
  end
end
