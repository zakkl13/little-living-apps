require "test_helper"

class HomeControllerTest < ActionDispatch::IntegrationTest
  test "root responds 200 and names the app" do
    get "/"
    assert_response :success
    assert_match(/Lilapp/, response.body)
  end

  test "greet greets by name" do
    get "/greet", params: { name: "Zakk" }
    assert_response :success
    assert_match(/Hello, Zakk!/, response.body)
  end
end
